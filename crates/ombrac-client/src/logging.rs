use std::ffi::c_char;
use std::sync::{Mutex, Once};

use tracing::{Level, Metadata};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

use ombrac_macros::warn;

use crate::config::LoggingConfig;

/// A type alias for the C-style callback function pointer.
///
/// The `level` parameter is an integer representation of the log level:
/// - `0`: TRACE
/// - `1`: DEBUG
/// - `2`: INFO
/// - `3`: WARN
/// - `4`: ERROR
pub type LogCallback = extern "C" fn(level: i32, message: *const c_char, target: *const c_char);

// A global, thread-safe handle to the registered log callback.
static LOG_CALLBACK: Mutex<Option<LogCallback>> = Mutex::new(None);
static LOGGING_INIT: Once = Once::new();

pub enum LoggingMode {
    Callback(LogCallback),
    Default(LoggingConfig),
}

pub fn init(mode: LoggingMode) {
    LOGGING_INIT.call_once(|| {
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

        match mode {
            LoggingMode::Callback(callback) => {
                set_log_callback(Some(callback));
                tracing_subscriber::registry()
                    .with(filter)
                    .with(FfiLayer)
                    .init();
            }
            LoggingMode::Default(config) => {
                let log_level = config
                    .log_level
                    .as_deref()
                    .map(|level_str| {
                        level_str.parse().unwrap_or_else(|_| {
                            warn!("Invalid log level '{}', defaulting to WARN", level_str);
                            tracing::Level::WARN
                        })
                    })
                    .unwrap_or(tracing::Level::WARN);

                let subscriber = tracing_subscriber::fmt()
                    .with_thread_ids(true)
                    .with_max_level(log_level);

                let (non_blocking, guard) = if let Some(path) = &config.log_dir {
                    let prefix = config
                        .log_prefix
                        .as_deref()
                        .unwrap_or_else(|| std::path::Path::new("log"));
                    let file_appender = tracing_appender::rolling::daily(path, prefix);
                    tracing_appender::non_blocking(file_appender)
                } else {
                    tracing_appender::non_blocking(std::io::stdout())
                };

                // The guard must be held for the lifetime of the program to ensure logs are flushed.
                // In a long-running application like this, we can "leak" it to achieve this.
                std::mem::forget(guard);
                subscriber.with_writer(non_blocking).init();
            }
        }
    });
}

/// A simple `tracing_subscriber` layer that forwards log records to a C callback.
pub struct FfiLayer;

impl<S> Layer<S> for FfiLayer
where
    S: tracing::Subscriber,
{
    fn enabled(
        &self,
        _metadata: &Metadata<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) -> bool {
        // We want to be enabled if a callback is set.
        LOG_CALLBACK.lock().unwrap().is_some()
    }

    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        // If a callback is registered, format the event and invoke the callback.
        if let Some(callback) = *LOG_CALLBACK.lock().unwrap() {
            let metadata = event.metadata();
            let level = metadata.level();
            let target = metadata.target();

            // A simple visitor to extract the `message` field from the event.
            struct MessageVisitor {
                message: String,
            }

            impl tracing::field::Visit for MessageVisitor {
                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
                    if field.name() == "message" {
                        self.message = format!("{:?}", value);
                    }
                }
            }

            let mut visitor = MessageVisitor {
                message: String::new(),
            };
            event.record(&mut visitor);

            // Convert the message and target to C-compatible strings.
            if let Ok(message_cstr) = std::ffi::CString::new(visitor.message)
                && let Ok(target_cstr) = std::ffi::CString::new(target) {
                    // Invoke the callback.
                    callback(
                        level_to_int(level),
                        message_cstr.as_ptr(),
                        target_cstr.as_ptr(),
                    );
                }
        }
    }
}

/// Sets the global log callback.
/// This function should be called by the FFI consumer before starting the service.
pub fn set_log_callback(callback: Option<LogCallback>) {
    *LOG_CALLBACK.lock().unwrap() = callback;
}

/// Converts a `tracing::Level` to a simple integer representation for FFI.
fn level_to_int(level: &Level) -> i32 {
    if *level == Level::ERROR {
        4
    } else if *level == Level::WARN {
        3
    } else if *level == Level::INFO {
        2
    } else if *level == Level::DEBUG {
        1
    } else if *level == Level::TRACE {
        0
    } else {
        -1 // Unknown
    }
}
