//! `anrelays` — command-line interface for the awesome-nostr-relays catalog.
//!
//! Subcommands:
//!
//! * `validate` — lint `relays.toml` and exit with a non-zero status on any
//!   schema / reference error. Intended for PR checks.
//! * `build` — regenerate `api/*.json` and refresh the `RELAYS:*` block in
//!   `README.md` from `relays.toml` and `health.json`.
//! * `check` — probe every relay (or a subset with `--limit`), merge results
//!   into `health.json`, and re-run `build`.

// The library target re-exports these crates indirectly; declare them as
// intentionally unused at the binary level so the `unused_crate_dependencies`
// lint does not misfire on deps consumed only via `awesome_nostr_relays::`.
#[allow(
    unused_imports,
    reason = "deps are consumed through the library re-exports"
)]
use {serde as _, serde_json as _, thiserror as _, tokio_tungstenite as _, toml as _, url as _};

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use awesome_nostr_relays::{
    health,
    probe::{self, ProbeConfig},
    render,
    source::{
        self, default_api_dir, default_health_path, default_readme_path, default_relays_path,
    },
    validate,
};
use clap::{Parser, Subcommand};
use futures::stream::{self, StreamExt};
use indicatif::{ProgressBar, ProgressStyle};
use tokio::sync::Mutex;
use tracing::{info, warn};

/// Top-level CLI argument parser.
#[derive(Parser, Debug)]
#[command(
    name = "anrelays",
    version,
    about = "Awesome Nostr Relays — curator CLI",
    long_about = None,
)]
struct Cli {
    /// Path to `relays.toml` (defaults to CWD/relays.toml).
    #[arg(long, global = true)]
    relays: Option<PathBuf>,

    /// Path to the health snapshot (defaults to CWD/health.json).
    #[arg(long, global = true)]
    health: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

/// All supported subcommands.
#[derive(Subcommand, Debug)]
enum Command {
    /// Lint the relay catalog.
    Validate,

    /// Regenerate `api/*.json` and the README data section.
    Build {
        /// Output directory for JSON artefacts (defaults to CWD/api).
        #[arg(long, alias = "dist")]
        output: Option<PathBuf>,
        /// README to rewrite (defaults to CWD/README.md).
        #[arg(long)]
        readme: Option<PathBuf>,
        /// Skip rewriting the README.
        #[arg(long)]
        skip_readme: bool,
    },

    /// Run liveness probes and persist `health.json`.
    Check {
        /// Hard per-relay timeout (seconds).
        #[arg(long, default_value_t = 10)]
        timeout: u64,
        /// Maximum number of concurrent probes.
        #[arg(long, default_value_t = 32)]
        concurrency: usize,
        /// Stop after this many relays (useful for smoke tests).
        #[arg(long)]
        limit: Option<usize>,
        /// Do not rewrite README / api/ after the check.
        #[arg(long)]
        no_build: bool,
    },
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    init_tracing();

    let cli = Cli::parse();
    let relays_path = cli.relays.unwrap_or_else(default_relays_path);
    let health_path = cli.health.unwrap_or_else(default_health_path);

    match cli.command {
        Command::Validate => run_validate(&relays_path),
        Command::Build {
            output,
            readme,
            skip_readme,
        } => run_build(
            &relays_path,
            &health_path,
            output.as_deref(),
            readme.as_deref(),
            skip_readme,
        ),
        Command::Check {
            timeout,
            concurrency,
            limit,
            no_build,
        } => {
            run_check(
                &relays_path,
                &health_path,
                Duration::from_secs(timeout),
                concurrency,
                limit,
                no_build,
            )
            .await
        }
    }
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

fn run_validate(relays_path: &Path) -> Result<()> {
    let dataset = source::load_dataset(relays_path)?;
    let summary = validate::validate(&dataset).map_err(anyhow::Error::from)?;
    info!(
        relays = summary.relay_count,
        collections = summary.collection_count,
        "validation ok"
    );
    for (id, count) in &summary.relays_per_collection {
        info!(collection = id.as_str(), relays = count, "collection");
    }
    Ok(())
}

fn run_build(
    relays_path: &Path,
    health_path: &Path,
    output: Option<&Path>,
    readme: Option<&Path>,
    skip_readme: bool,
) -> Result<()> {
    let dataset = source::load_dataset(relays_path)?;
    validate::validate(&dataset).map_err(anyhow::Error::from)?;

    let health = source::load_health(health_path)?;
    let api_dir = output.map_or_else(default_api_dir, Path::to_path_buf);
    let readme_path = readme.map_or_else(default_readme_path, Path::to_path_buf);

    render::json::write_all(&dataset, &health, &api_dir)?;
    info!(dir = %api_dir.display(), "wrote api/*.json");

    if !skip_readme {
        render::markdown::update_readme(&readme_path, &dataset, &health)?;
        info!(path = %readme_path.display(), "updated README");
    }
    Ok(())
}

async fn run_check(
    relays_path: &Path,
    health_path: &Path,
    timeout: Duration,
    concurrency: usize,
    limit: Option<usize>,
    no_build: bool,
) -> Result<()> {
    let dataset = source::load_dataset(relays_path)?;
    validate::validate(&dataset).map_err(anyhow::Error::from)?;

    let mut report = source::load_health(health_path)?;
    let known_urls: Vec<String> = dataset
        .relays
        .iter()
        .map(|r| r.url.as_str().to_owned())
        .collect();
    health::prune_orphans(&mut report, &known_urls);

    let targets: Vec<_> = match limit {
        Some(n) => dataset.relays.iter().take(n).collect(),
        None => dataset.relays.iter().collect(),
    };

    info!(
        total = targets.len(),
        concurrency = concurrency,
        timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
        "probing relays",
    );

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(format!(
            "awesome-nostr-relays/{}",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .context("build reqwest client")?;

    let config = ProbeConfig {
        timeout,
        http_timeout: timeout.min(Duration::from_secs(8)),
    };

    let total = u64::try_from(targets.len()).unwrap_or(u64::MAX);
    let progress = build_progress_bar(total);
    let report = Arc::new(Mutex::new(report));
    let http = Arc::new(http);

    stream::iter(targets.into_iter().map(|relay| {
        let report = Arc::clone(&report);
        let http = Arc::clone(&http);
        let progress = progress.clone();
        async move {
            let url = relay.url.clone();
            let outcome = probe::probe(&http, &url, config).await;
            let mut guard = report.lock().await;
            match outcome {
                Ok(ok) => health::record_success(&mut guard, url.as_str(), &ok),
                Err(e) => {
                    let msg = e.to_string();
                    warn!(%url, error = %msg, "probe failed");
                    health::record_failure(&mut guard, url.as_str(), &msg);
                }
            }
            progress.inc(1);
        }
    }))
    .buffer_unordered(concurrency)
    .collect::<()>()
    .await;

    progress.finish_with_message("done");

    let mut report = Arc::try_unwrap(report)
        .map_err(|_| anyhow::anyhow!("report arc still shared"))?
        .into_inner();
    report.last_run = Some(chrono::Utc::now());
    source::save_health(health_path, &report)?;
    info!(path = %health_path.display(), "wrote health snapshot");

    if !no_build {
        run_build(relays_path, health_path, None, None, false)?;
    }

    Ok(())
}

fn build_progress_bar(total: u64) -> ProgressBar {
    let bar = ProgressBar::new(total);
    if let Ok(style) = ProgressStyle::with_template("{bar:40.cyan/blue} {pos}/{len} {msg}") {
        bar.set_style(style.progress_chars("=>-"));
    }
    bar
}
