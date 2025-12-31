use std::ffi::{CStr, c_char};
use std::sync::{Arc, Mutex};

use tokio::runtime::{Builder, Runtime};

#[cfg(feature = "tracing")]
use ombrac_macros::{error, info};

use crate::config::{ServiceConfig, load_from_json};
#[cfg(feature = "tracing")]
use crate::logging::LogCallback;
use crate::service::OmbracServer;

// A global, thread-safe handle to the running service instance.
static SERVICE_HANDLE: Mutex<Option<ServiceHandle>> = Mutex::new(None);

// Encapsulates the service instance and its associated Tokio runtime.
struct ServiceHandle {
    service: Option<OmbracServer>,
    runtime: Runtime,
}

/// A helper function to safely convert a C string pointer to a Rust String.
/// Returns an empty string if the pointer is null.
///
/// This function immediately copies the C string to avoid lifetime issues.
/// This is safer than returning a reference, as it doesn't require the caller
/// to ensure the C string remains valid.
///
/// # Safety
///
/// The caller must ensure that `s` points to a valid null-terminated C string
/// at the time of the call. The string is immediately copied, so the C string
/// can be freed after this function returns.
unsafe fn c_str_to_string(s: *const c_char) -> String {
    if s.is_null() {
        return String::new();
    }
    // Safety: We assume the C string is valid and null-terminated at this point.
    // We immediately convert to an owned String, so lifetime issues are avoided.
    unsafe { CStr::from_ptr(s).to_str().unwrap_or("").to_string() }
}

/// Initializes the logging system to use a C-style callback for log messages.
///
/// This function must be called before `ombrac_server_service_startup` if you wish to
/// receive logs in a C-compatible way. It sets up a global logger that will
/// forward all log records to the provided callback function.
///
/// # Arguments
///
/// * `callback` - A function pointer of type `LogCallback`. The callback receives
///   a buffer pointer and length. The buffer is valid only during the callback call
///   and must be copied if needed for later use.
///
/// # Safety
///
/// The provided `callback` function pointer must be valid and remain valid for
/// the lifetime of the program. If a null pointer is passed, logging will be
/// disabled.
///
/// The callback must not modify the buffer and should not call back into Rust
/// code that might trigger logging, as this could cause deadlocks.
///
/// This function is protected against Rust panics crossing the FFI boundary.
#[cfg(feature = "tracing")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ombrac_server_set_log_callback(callback: *const LogCallback) {
    let _ = std::panic::catch_unwind(|| {
        let callback = if callback.is_null() {
            None
        } else {
            Some(unsafe { *callback })
        };
        crate::logging::set_log_callback(callback);
    });

    // If panic occurred, log it but don't propagate (FFI boundary safety).
    // Note: We can't return an error code here as the original API was void.
    // The callback will simply not be set if a panic occurs.
}

/// Initializes and starts the service with a given JSON configuration.
///
/// This function sets up the asynchronous runtime, parses the configuration,
/// and launches the main service. It must be called before any other service
/// operations. The service must be shut down via `ombrac_server_service_shutdown` to ensure
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
/// called concurrently with `ombrac_server_service_shutdown`.
///
/// This function is protected against Rust panics crossing the FFI boundary.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ombrac_server_service_startup(config_json: *const c_char) -> i32 {
    // Catch panics to prevent them from crossing the FFI boundary (undefined behavior).
    let result = std::panic::catch_unwind(|| {
        // Convert C string to owned Rust String immediately to avoid lifetime issues.
        let config_str = unsafe { c_str_to_string(config_json) };

        // Parse the JSON immediately to avoid holding references to the C string.
        let service_config: ServiceConfig = match load_from_json(&config_str) {
            Ok(cfg) => cfg,
            Err(e) => {
                #[cfg(feature = "tracing")]
                error!("Failed to parse config JSON: {}", e);
                return -1;
            }
        };

        #[cfg(feature = "tracing")]
        crate::logging::init_for_ffi(&service_config.logging);

        let runtime = match Builder::new_multi_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(_e) => {
                #[cfg(feature = "tracing")]
                error!("Failed to create Tokio runtime: {_e}");
                return -1;
            }
        };

        // Use block_on with panic protection.
        // Note: If we're already in an async context, this could cause issues,
        // but for FFI entry points, this should be safe.
        let service =
            runtime.block_on(async { OmbracServer::build(Arc::new(service_config)).await });

        let service = match service {
            Ok(s) => s,
            Err(e) => {
                #[cfg(feature = "tracing")]
                error!("Failed to build service: {}", e);
                return -1;
            }
        };

        let mut handle_guard = SERVICE_HANDLE.lock().unwrap();
        if handle_guard.is_some() {
            #[cfg(feature = "tracing")]
            error!("Service is already running. Please shut down the existing service first.");
            return -1;
        }

        *handle_guard = Some(ServiceHandle {
            service: Some(service),
            runtime,
        });

        #[cfg(feature = "tracing")]
        info!("Service started successfully");

        0
    });

    match result {
        Ok(ret) => ret,
        Err(_) => {
            // Panic occurred. Log if possible, but don't panic again.
            #[cfg(feature = "tracing")]
            error!("Panic occurred in ombrac_server_service_startup");
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
/// `ombrac_server_service_startup`.
///
/// This function is protected against Rust panics crossing the FFI boundary.
#[unsafe(no_mangle)]
pub extern "C" fn ombrac_server_service_shutdown() -> i32 {
    // Catch panics to prevent them from crossing the FFI boundary (undefined behavior).
    let result = std::panic::catch_unwind(|| {
        let mut handle_guard = SERVICE_HANDLE.lock().unwrap();

        if let Some(mut handle) = handle_guard.take() {
            #[cfg(feature = "tracing")]
            info!("Shutting down service");

            if let Some(service) = handle.service.take() {
                // Use block_on with panic protection.
                let shutdown_result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        handle.runtime.block_on(async {
                            service.shutdown().await;
                        });
                    }));

                if shutdown_result.is_err() {
                    #[cfg(feature = "tracing")]
                    error!("Panic occurred during service shutdown");
                }
            }

            handle.runtime.shutdown_background();

            #[cfg(feature = "tracing")]
            info!("Service shut down complete.");
        } else {
            #[cfg(feature = "tracing")]
            info!("Service was not running.");
        }

        // Shutdown logging to release the guard and flush remaining logs.
        #[cfg(feature = "tracing")]
        crate::logging::shutdown_logging();

        0
    });

    match result {
        Ok(ret) => ret,
        Err(_) => {
            // Panic occurred. Log if possible, but don't panic again.
            #[cfg(feature = "tracing")]
            error!("Panic occurred in ombrac_server_service_shutdown");
            // Still try to shutdown logging even if there was a panic.
            #[cfg(feature = "tracing")]
            let _ = std::panic::catch_unwind(|| crate::logging::shutdown_logging());
            0
        }
    }
}

/// Returns the version of the ombrac-server library.
///
/// The returned string is a null-terminated UTF-8 string. The memory for this
/// string is managed by the library and should not be freed by the caller.
#[unsafe(no_mangle)]
pub extern "C" fn ombrac_server_get_version() -> *const c_char {
    const VERSION_WITH_NULL: &str = concat!(env!("CARGO_PKG_VERSION"), "\0");
    VERSION_WITH_NULL.as_ptr() as *const c_char
}
