//! Write [Icinga Notifications channel plugins](https://github.com/Icinga/icinga-notifications/blob/main/doc/10-Channels.md)
//! on top of `jsonrpc-rs`.

pub mod channel;
pub mod config;
pub mod delivery;
pub mod error;
pub mod info;
// Used by `run_with` from the next step on.
#[cfg_attr(not(test), allow(dead_code))]
mod methods;
pub mod notification;
// Used by `run_with` from the next steps on.
#[cfg_attr(not(test), allow(dead_code))]
mod output;
pub mod state;
