//! Write [Icinga Notifications channel plugins](https://github.com/Icinga/icinga-notifications/blob/main/doc/10-Channels.md)
//! on top of `jsonrpc-rs`.

pub mod channel;
pub mod config;
pub mod delivery;
pub mod error;
pub mod info;
pub mod log;
pub mod notification;
mod run;
pub mod state;

mod methods;
mod output;

pub use run::run_with;
