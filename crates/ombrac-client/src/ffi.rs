use std::ffi::{CStr, c_char};
use std::sync::Mutex;

use tokio::runtime::{Builder, Runtime};

use ombrac_macros::{error, info};

use crate::OmbracClient;
#[cfg(feature = "tracing")]
use crate::logging::LogCallback;

// A global, thread-safe handle to the running service instance.
static SERVICE_HANDLE: Mutex<Option<ServiceHandle>> = Mutex::new(None);

// Encapsulates the service instance and its associated Tokio runtime.
struct ServiceHandle {
    service: Option<Box<OmbracClient>>,
    runtime: Runtime,
}

/// A helper function to safely convert a C string pointer to an owned Rust String.
/// Returns an empty string if the pointer is null or contains invalid UTF-8.
///
/// The string is immediately copied to avoid lifetime issues with the C string.
unsafe fn c_str_to_string(s: *const c_char) -> String {
    if s.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(s).to_str().unwrap_or("").to_string() }
}

/// Initializes the logging system to use a C-style callback for log messages.
///
/// This function must be called before `ombrac_client_service_startup` if you wish to
/// receive logs in a C-compatible way. It sets up a global logger that will
/// forward all log records to the provided callback function.
///
/// # Arguments
///
/// * `callback` - A function pointer of type `LogCallback`. See the definition of
///   `LogCallback` for the expected signature and log level mappings.
///
/// # Safety
///
/// The provided `callback` function pointer must be valid and remain valid for
/// the lifetime of the program. If a null pointer is passed, logging will be
/// disabled.
#[cfg(feature = "tracing")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ombrac_client_set_log_callback(callback: *const LogCallback) {
    let _ = std::panic::catch_unwind(|| {
        let callback = if callback.is_null() {
            None
        } else {
            Some(unsafe { *callback })
        };
        crate::logging::set_log_callback(callback);
    });
}

/// Initializes and starts the service with a given JSON configuration.
///
/// This function sets up the asynchronous runtime, parses the configuration,
/// and launches the main service. It must be called before any other service
/// operations. The service must be shut down via `ombrac_client_service_shutdown` to ensure
/// a clean exit.
///
/// # Arguments
///
/// * `config_json` - A pointer to a null-terminated UTF-8 string containing the
///   service configuration in JSON format.
///
/// # Returns
///
/// * `0` on success.
/// * `-1` on failure (e.g., invalid configuration, service already running, or
///   runtime initialization failed).
///
/// # Safety
///
/// The caller must ensure that `config_json` is a valid pointer to a
/// null-terminated C string. This function is not thread-safe and should not be
/// called concurrently with `ombrac_client_service_shutdown`.
///
/// This function is protected against Rust panics crossing the FFI boundary.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ombrac_client_service_startup(config_json: *const c_char) -> i32 {
    let result = std::panic::catch_unwind(|| {
        let config_str = unsafe { c_str_to_string(config_json) };

        let service_config = match crate::config::load_from_json(&config_str) {
            Ok(cfg) => cfg,
            Err(e) => {
                error!("Failed to parse config JSON: {}", e);
                return -1;
            }
        };

        #[cfg(feature = "tracing")]
        crate::logging::init_for_ffi(&service_config.logging);

        let runtime = match Builder::new_multi_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(_e) => {
                error!("Failed to create Tokio runtime: {}", _e);
                return -1;
            }
        };

        let service = runtime.block_on(async {
            use std::sync::Arc;
            OmbracClient::build(Arc::new(service_config)).await
        });

        let service = match service {
            Ok(s) => s,
            Err(e) => {
                error!("Failed to build service: {}", e);
                return -1;
            }
        };

        let mut handle_guard = SERVICE_HANDLE.lock().unwrap_or_else(|e| e.into_inner());
        if handle_guard.is_some() {
            error!("Service is already running. Please shut down the existing service first.");
            return -1;
        }

        *handle_guard = Some(ServiceHandle {
            service: Some(Box::new(service)),
            runtime,
        });

        info!("Service started successfully");

        0
    });

    match result {
        Ok(ret) => ret,
        Err(_) => {
            error!("Panic occurred in ombrac_client_service_startup");
            -1
        }
    }
}

/// Triggers a network rebind on the underlying transport.
///
/// This is useful in scenarios where the network environment changes,
/// to ensure the client can re-establish its connection through a new socket.
///
/// # Returns
///
/// * `0` on success.
/// * `-1` if the service is not running or the rebind operation fails.
///
/// # Safety
///
/// This function is not thread-safe and should not be called concurrently with
/// other service management functions.
#[unsafe(no_mangle)]
pub extern "C" fn ombrac_client_service_rebind() -> i32 {
    let result = std::panic::catch_unwind(|| {
        let handle_guard = SERVICE_HANDLE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(handle) = handle_guard.as_ref() {
            if let Some(service) = &handle.service {
                let result = handle.runtime.block_on(service.rebind());
                if let Err(e) = result {
                    error!("Failed to rebind: {}", e);
                    return -1;
                } else {
                    info!("Service rebind successful");
                    return 0;
                }
            }
        }
        -1
    });

    match result {
        Ok(ret) => ret,
        Err(_) => {
            error!("Panic occurred in ombrac_client_service_rebind");
            -1
        }
    }
}

/// Shuts down the running service and releases all associated resources.
///
/// This function will gracefully stop the service and terminate the asynchronous
/// runtime. It is safe to call even if the service was not started or has
/// already been stopped.
///
/// # Returns
///
/// * `0` on completion.
///
/// # Safety
///
/// This function is not thread-safe and should not be called concurrently with
/// `ombrac_client_service_startup`.
///
/// This function is protected against Rust panics crossing the FFI boundary.
#[unsafe(no_mangle)]
pub extern "C" fn ombrac_client_service_shutdown() -> i32 {
    let result = std::panic::catch_unwind(|| {
        let mut handle_guard = SERVICE_HANDLE.lock().unwrap_or_else(|e| e.into_inner());

        if let Some(mut handle) = handle_guard.take() {
            info!("Shutting down service");

            if let Some(service) = handle.service.take() {
                let shutdown_result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        handle.runtime.block_on(async {
                            service.shutdown().await;
                        });
                    }));

                if shutdown_result.is_err() {
                    error!("Panic occurred during service shutdown");
                }
            }

            handle.runtime.shutdown_background();

            info!("Service shut down complete.");
        } else {
            info!("Service was not running.");
        }

        #[cfg(feature = "tracing")]
        crate::logging::shutdown_logging();

        0
    });

    match result {
        Ok(ret) => ret,
        Err(_) => {
            error!("Panic occurred in ombrac_client_service_shutdown");
            #[cfg(feature = "tracing")]
            let _ = std::panic::catch_unwind(|| crate::logging::shutdown_logging());
            0
        }
    }
}

/// Returns the version of the ombrac-client library.
///
/// The returned string is a null-terminated UTF-8 string. The memory for this
/// string is managed by the library and should not be freed by the caller.
#[unsafe(no_mangle)]
pub extern "C" fn ombrac_client_get_version() -> *const c_char {
    const VERSION_WITH_NULL: &str = concat!(env!("CARGO_PKG_VERSION"), "\0");
    VERSION_WITH_NULL.as_ptr() as *const c_char
}
