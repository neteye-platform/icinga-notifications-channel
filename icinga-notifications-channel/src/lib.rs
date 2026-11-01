//! Write [Icinga Notifications channel plugins](https://github.com/Icinga/icinga-notifications/blob/main/doc/10-Channels.md)
//! on top of `jsonrpc-rs`.

pub mod channel;
pub mod config;
pub mod delivery;
pub mod error;
pub mod info;
pub mod log;
pub mod notification;
pub mod state;

// The modules below are wired together by `run_with` in the next steps.
#[cfg_attr(not(test), allow(dead_code))]
mod methods;
#[cfg_attr(not(test), allow(dead_code))]
mod output;
