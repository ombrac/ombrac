#[macro_export]
macro_rules! trace {
    ($fmt:expr $(, $args:expr)*) => {
        #[cfg(debug_assertions)]
        #[cfg(feature = "trace")]
        {
            tracing::trace!($fmt $(, $args)*);
        }
    };
}

#[macro_export]
macro_rules! debug {
    ($fmt:expr $(, $args:expr)*) => {
        #[cfg(debug_assertions)]
        #[cfg(feature = "trace")]
        {
            tracing::debug!($fmt $(, $args)*);
        }
    };
}

#[macro_export]
macro_rules! info {
    ($fmt:expr $(, $args:expr)*) => {
        #[cfg(feature = "trace")]
        {
            tracing::info!($fmt $(, $args)*);
        }
    };
}

#[macro_export]
macro_rules! warn {
    ($fmt:expr $(, $args:expr)*) => {
        #[cfg(feature = "trace")]
        {
            tracing::warn!($fmt $(, $args)*);
        }
    };
}

#[macro_export]
macro_rules! error {
    ($fmt:expr $(, $args:expr)*) => {
        #[cfg(feature = "trace")]
        {
            tracing::error!($fmt $(, $args)*);
        }
    };
}
