use std::io;

fn main() {
    let config = match ombrac_server::config::load() {
        Ok(cfg) => cfg,
        Err(error) => {
            eprintln!("failed to load configuration: {}", error);
            std::process::exit(1);
        }
    };

    // Initialize logging for the binary.
    #[cfg(feature = "tracing")]
    ombrac_server::logging::init_for_binary(&config.logging);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    if let Err(e) = rt.block_on(run_from_cli(config)) {
        eprintln!("fatal error occurred during runtime: {}", e);
        std::process::exit(1);
    }
}

/// A high-level function to run the server from a command-line context.
/// It builds the service, waits for a Ctrl+C signal, and then gracefully shuts down.
pub async fn run_from_cli(config: ombrac_server::config::ServiceConfig) -> io::Result<()> {
    use ombrac_server::service::OmbracServer;
    use std::sync::Arc;

    match OmbracServer::build(Arc::new(config)).await {
        Ok(server) => {
            tokio::signal::ctrl_c().await?;
            server.shutdown().await;
            Ok(())
        }
        Err(e) => Err(io::Error::other(e.to_string())),
    }
}
