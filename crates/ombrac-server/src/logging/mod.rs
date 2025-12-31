#[cfg(feature = "binary")]
pub mod binary;

#[cfg(feature = "ffi")]
pub mod ffi;

// Re-export functions for backward compatibility
#[cfg(feature = "binary")]
pub use binary::init_for_binary;

#[cfg(feature = "ffi")]
pub use ffi::{LogCallback, init_for_ffi, set_log_callback, shutdown_logging};
