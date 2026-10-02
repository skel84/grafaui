//! One Prometheus endpoint. Query construction and response conversion are pure;
//! only `Client` performs I/O. Calls are blocking and must run off the UI thread.
mod client;
pub mod lifecycle;
pub mod query;
pub mod response;

pub use client::Client;
pub use query::Variables;

pub type Result<T> = std::result::Result<T, String>;
