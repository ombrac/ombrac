#[macro_export]
macro_rules! trace {
    ($fmt:expr $(, $args:expr)*) => {
        #[cfg(debug_assertions)]
        #[cfg(feature = "tracing")]
        {
            tracing::trace!($fmt $(, $args)*);
        }
    };
}

#[macro_export]
macro_rules! debug {
    ($fmt:expr $(, $args:expr)*) => {
        #[cfg(debug_assertions)]
        #[cfg(feature = "tracing")]
        {
            tracing::debug!($fmt $(, $args)*);
        }
    };
}

#[macro_export]
macro_rules! info {
    ($fmt:expr $(, $args:expr)*) => {
        #[cfg(feature = "tracing")]
        {
            tracing::info!($fmt $(, $args)*);
        }
    };
}

#[macro_export]
macro_rules! warn {
    ($fmt:expr $(, $args:expr)*) => {
        #[cfg(feature = "tracing")]
        {
            tracing::warn!($fmt $(, $args)*);
        }
    };
}

#[macro_export]
macro_rules! error {
    ($fmt:expr $(, $args:expr)*) => {
        #[cfg(feature = "tracing")]
        {
            tracing::error!($fmt $(, $args)*);
        }
    };
}

#[macro_export]
macro_rules! try_or_continue {
    ($expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(_error) => {
                #[cfg(feature = "tracing")]
                {
                    tracing::error!("{}", _error);
                }

                continue;
            }
        }
    };
}

#[macro_export]
macro_rules! try_or_break {
    ($expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(_error) => {
                #[cfg(feature = "tracing")]
                {
                    tracing::error!("{}", _error);
                }

                break;
            }
        }
    };
}

#[macro_export]
macro_rules! try_or_return {
    ($expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(_error) => {
                #[cfg(feature = "tracing")]
                {
                    tracing::error!("{}", _error);
                }

                return;
            }
        }
    };
}

#[macro_export]
macro_rules! debug_timer {
    ($name:expr, $body:expr) => {{
        #[cfg(debug_assertions)]
        #[cfg(feature = "tracing")]
        let start = std::time::Instant::now();

        let result = $body;

        #[cfg(debug_assertions)]
        #[cfg(feature = "tracing")]
        tracing::debug!("{} {:?}", $name, start.elapsed());

        result
    }};
}
