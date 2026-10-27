//! Clicker: a scriptable Apple TV remote for Linux.

pub mod app;
pub mod companion;
pub mod crypto;
pub mod discovery;
pub mod error;
pub mod guide;
pub mod hap;
pub mod log;
pub mod opack;
pub mod output;
pub mod setup;
pub mod srp;
pub mod store;
pub mod text;
pub mod tlv8;
pub mod tui;

pub use error::{Error, Result};
