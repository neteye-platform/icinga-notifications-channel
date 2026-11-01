#![doc = include_str!("../README.md")]

pub mod server;

#[cfg(feature = "tokio_util")]
pub mod tokio;

#[cfg(feature = "macros")]
pub use jsonrpc_rs_macros::jsonrpc;

#[doc(hidden)]
pub use serde;
