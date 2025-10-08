use std::ffi::c_char;
use std::io::Write;
use std::sync::{Arc, OnceLock};

use arc_swap::ArcSwap;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Registry};

use crate::config::LoggingConfig;

/// A type alias for the C-style callback function pointer.
pub type LogCallback = extern "C" fn(message: *const c_char);

// A global, thread-safe, and lock-free handle to the registered log callback.
static LOG_CALLBACK: OnceLock<ArcSwap<Option<LogCallback>>> = OnceLock::new();

/// Returns a reference to the global `ArcSwap<Option<LogCallback>>`.
/// Initializes it on first access.
fn get_log_callback_handle() -> &'static ArcSwap<Option<LogCallback>> {
    LOG_CALLBACK.get_or_init(|| ArcSwap::from(Arc::new(None)))
}

/// A custom writer that forwards formatted log messages to the FFI callback.
struct FfiWriter;

impl Write for FfiWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let callback_handle = get_log_callback_handle();
        let callback_arc = callback_handle.load();
        if let Some(callback) = **callback_arc {
            // Trim trailing newline, as the callback consumer might add its own.
            let message_bytes = if buf.last() == Some(&b'\n') {
                &buf[..buf.len() - 1]
            } else {
                buf
            };

            if let Ok(message_cstr) = std::ffi::CString::new(message_bytes) {
                callback(message_cstr.as_ptr());
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Initializes logging for a standalone binary application.
///
/// This setup directs logs to `stdout`.
pub fn init_for_binary(config: &LoggingConfig) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(config.log_level.as_deref().unwrap_or("info")));

    let (non_blocking_writer, _guard) = tracing_appender::non_blocking(std::io::stdout());
    std::mem::forget(_guard); // Keep the guard alive for the duration of the program.

    let layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking_writer)
        .with_thread_ids(true);

    Registry::default().with(filter).with(layer).init();
}

/// Initializes logging for FFI consumers.
///
/// This setup directs logs to the registered FFI callback.
pub fn init_for_ffi(config: &LoggingConfig) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(config.log_level.as_deref().unwrap_or("info")));

    let (ffi_writer, _guard) = tracing_appender::non_blocking(FfiWriter);
    std::mem::forget(_guard); // Keep the guard alive.

    let layer = tracing_subscriber::fmt::layer()
        .with_writer(ffi_writer)
        .with_ansi(false) // ANSI codes are not expected by FFI consumers.
        .with_target(true)
        .with_thread_ids(true)
        .with_level(true);

    Registry::default().with(filter).with(layer).init();
}

/// Sets or clears the global log callback for FFI.
///
/// This function should be called before `init_for_ffi`.
pub fn set_log_callback(callback: Option<LogCallback>) {
    let callback_handle = get_log_callback_handle();
    callback_handle.store(Arc::new(callback));
}
