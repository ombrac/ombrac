#[cfg(feature = "tracing")]
pub use tracing::{Level, debug, error, info, info_span, span, trace, warn};

#[cfg(not(feature = "tracing"))]
#[macro_export]
macro_rules! trace {
    ($($arg:tt)*) => {{}};
}

#[cfg(not(feature = "tracing"))]
#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {{}};
}

#[cfg(not(feature = "tracing"))]
#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {{}};
}

#[cfg(not(feature = "tracing"))]
#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => {{}};
}

#[cfg(not(feature = "tracing"))]
#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => {{}};
}

#[cfg(not(feature = "tracing"))]
#[macro_export]
macro_rules! span {
    ($level:expr, $($arg:tt)*) => {
        $crate::logging::NoOpSpan
    };
}

#[cfg(not(feature = "tracing"))]
#[macro_export]
macro_rules! info_span {
    ($($arg:tt)*) => {{}};
}
