use std::ffi::{CStr, c_char};
use std::sync::{Arc, Mutex};

use figment::Figment;
use figment::providers::{Format, Json, Serialized};
use tokio::runtime::{Builder, Runtime};

use ombrac_macros::{error, info};

use crate::config::{ConfigFile, ServiceConfig};
use crate::logging::{self, LogCallback, LoggingMode};
#[cfg(feature = "transport-quic")]
use crate::service::QuicServiceBuilder;
use crate::service::Service;

// A global, thread-safe handle to the running service instance.
static SERVICE_HANDLE: Mutex<Option<ServiceHandle>> = Mutex::new(None);

// Encapsulates the service instance and its associated Tokio runtime.
struct ServiceHandle {
    #[cfg(feature = "transport-quic")]
    service: Option<
        Box<Service<ombrac_transport::quic::server::Server, ombrac_transport::quic::Connection>>,
    >,
    runtime: Runtime,
}

/// A helper function to safely convert a C string pointer to a Rust string slice.
/// Returns an empty string if the pointer is null.
unsafe fn c_str_to_str<'a>(s: *const c_char) -> &'a str {
    if s.is_null() {
        return "";
    }
    unsafe { CStr::from_ptr(s).to_str().unwrap_or("") }
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
#[cfg(feature = "tracing")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ombrac_server_logging_init(callback: LogCallback) {
    logging::init(LoggingMode::Callback(callback));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ombrac_server_service_startup(config_json: *const c_char) -> i32 {
    let config_str = unsafe { c_str_to_str(config_json) };

    let config_file: ConfigFile = match Figment::new()
        .merge(Serialized::defaults(ConfigFile::default()))
        .merge(Json::string(config_str))
        .extract()
    {
        Ok(cfg) => cfg,
        Err(_e) => {
            error!("Failed to parse config JSON: {_e}");
            return -1;
        }
    };

    let service_config = match (config_file.secret, config_file.listen) {
        (Some(secret), Some(listen)) => ServiceConfig {
            secret,
            listen,
            #[cfg(feature = "transport-quic")]
            transport: config_file.transport,
            #[cfg(feature = "tracing")]
            logging: config_file.logging,
        },
        (None, _) => {
            error!("Configuration error: missing required field `secret` in JSON config");
            return -1;
        }
        (_, None) => {
            error!("Configuration error: missing required field `listen` in JSON config");
            return -1;
        }
    };

    let runtime = match Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(_e) => {
            error!("Failed to create Tokio runtime: {_e}");
            return -1;
        }
    };

    let service = runtime.block_on(async {
        #[cfg(feature = "transport-quic")]
        Service::build::<QuicServiceBuilder>(Arc::new(service_config)).await
    });

    #[cfg(feature = "transport-quic")]
    let service = match service {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to build service: {}", e);
            return -1;
        }
    };

    let mut handle_guard = SERVICE_HANDLE.lock().unwrap();
    if handle_guard.is_some() {
        error!("Service is already running. Please shut down the existing service first.");
        return -1;
    }

    *handle_guard = Some(ServiceHandle {
        #[cfg(feature = "transport-quic")]
        service: Some(Box::new(service)),
        runtime,
    });

    info!("Service started successfully");

    0
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
#[unsafe(no_mangle)]
pub extern "C" fn ombrac_server_service_shutdown() -> i32 {
    let mut handle_guard = SERVICE_HANDLE.lock().unwrap();

    if let Some(mut handle) = handle_guard.take() {
        info!("Shutting down service");

        #[cfg(feature = "transport-quic")]
        if let Some(service) = handle.service.take() {
            handle.runtime.block_on(async {
                service.shutdown().await;
            });
        }

        handle.runtime.shutdown_background();

        info!("Service shut down complete.");
    } else {
        info!("Service was not running.");
    }

    0
}
