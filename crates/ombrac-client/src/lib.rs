pub mod client;
pub mod config;
pub mod connection;
#[cfg(any(
    feature = "endpoint-default",
    feature = "endpoint-socks",
    feature = "endpoint-http",
    feature = "endpoint-tun"
))]
pub mod endpoint;
#[cfg(feature = "ffi")]
pub mod ffi;
#[cfg(feature = "tracing")]
pub mod logging;
pub mod service;

// Re-export commonly used types for convenience
pub use config::{EndpointConfig, ServiceConfig, TransportConfig};
pub use service::{Error as ServiceError, OmbracClient, Result as ServiceResult};
