//! Write [Icinga Notifications channel plugins](https://github.com/Icinga/icinga-notifications/blob/main/doc/10-Channels.md)
//! on top of `jsonrpc-rs`.

pub mod config;
pub mod delivery;
pub mod error;
pub mod info;
pub mod notification;
pub mod state;
