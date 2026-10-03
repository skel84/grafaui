//! One Prometheus endpoint. Query construction, variable resolution
//! ([`plan::VariablePlan`]) and response conversion are pure, and every call
//! is an [`api::ApiRequest`] that any transport can send. Only `Client`, behind
//! the default `client` feature, performs I/O. Its calls are blocking and must
//! run off the UI thread.
pub mod api;
#[cfg(feature = "client")]
mod client;
pub mod lifecycle;
pub mod plan;
pub mod query;
pub mod response;

#[cfg(feature = "client")]
pub use client::Client;
pub use query::Variables;

pub type Result<T> = std::result::Result<T, String>;
