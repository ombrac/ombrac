use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Registry};

use crate::config::LoggingConfig;

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
