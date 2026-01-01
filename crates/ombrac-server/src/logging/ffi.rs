use std::ffi::c_char;
use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock};

use arc_swap::ArcSwap;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Registry};

use crate::config::LoggingConfig;

/// A type alias for the C-style callback function pointer.
/// Changed to pass (buf, len) instead of a null-terminated string for better performance.
pub type LogCallback = extern "C" fn(buf: *const c_char, len: usize);

// A global, thread-safe, and lock-free handle to the registered log callback.
static LOG_CALLBACK: OnceLock<ArcSwap<Option<LogCallback>>> = OnceLock::new();

// A global guard for the tracing appender to prevent memory leaks.
// The guard must be kept alive for the lifetime of the logging system.
static LOG_GUARD: Mutex<Option<WorkerGuard>> = Mutex::new(None);

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

            // Directly pass the buffer pointer and length to avoid CString allocation.
            // The callback must not modify the buffer and should copy it if needed.
            // Safety: The buffer is valid for the duration of the callback call.
            callback(message_bytes.as_ptr() as *const c_char, message_bytes.len());
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Initializes logging for FFI consumers.
///
/// This setup directs logs to the registered FFI callback.
///
/// # Safety
///
/// This function should only be called once. If called multiple times, it will
/// replace the previous guard, which may cause the previous logging system to
/// stop flushing properly. The guard is stored globally and will be dropped
/// when `shutdown_logging` is called.
pub fn init_for_ffi(config: &LoggingConfig) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(config.log_level.as_deref().unwrap_or("info")));

    let (ffi_writer, guard) = tracing_appender::non_blocking(FfiWriter);

    // Store the guard in a global Mutex to prevent memory leaks.
    // The guard will be dropped when shutdown_logging is called.
    let mut guard_handle = LOG_GUARD.lock().unwrap();
    *guard_handle = Some(guard);

    let layer = tracing_subscriber::fmt::layer()
        .with_writer(ffi_writer)
        .with_ansi(false) // ANSI codes are not expected by FFI consumers.
        .with_target(true)
        .with_thread_ids(true)
        .with_level(true);

    Registry::default().with(filter).with(layer).init();
}

/// Shuts down the logging system and releases the guard.
///
/// This function should be called during service shutdown to ensure all logs
/// are flushed and resources are properly released.
pub fn shutdown_logging() {
    let mut guard_handle = LOG_GUARD.lock().unwrap();
    // Dropping the guard will flush the buffer and stop the background thread.
    *guard_handle = None;
}

/// Sets or clears the global log callback for FFI.
///
/// This function should be called before `init_for_ffi`.
pub fn set_log_callback(callback: Option<LogCallback>) {
    let callback_handle = get_log_callback_handle();
    callback_handle.store(Arc::new(callback));
}
