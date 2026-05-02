//! Output renderers.
//!
//! [`json`] produces the machine-readable `dist/*.json` files. [`markdown`]
//! regenerates the auto-managed section of the top-level README between the
//! start / end markers.

pub mod json;
pub mod markdown;
