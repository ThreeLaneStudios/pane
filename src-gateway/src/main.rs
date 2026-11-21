use clap::{Parser, Subcommand};
use pane_gateway::{
    agents::AgentRegistry,
    allowlist::Allowlist,
    auth::AuthState,
    config::Config,
    error::GatewayError,
    metrics::Metrics,
    oc_client::OcClient,
    param_compat,
    server::{build_tls_config, start_server},
    tls::TlsManager,
};
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "pane-gateway", version, about = "Pane Gateway server")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate a fresh pairing code (requires running gateway)
    PairingCode,
}

#[tokio::main]
async fn main() -> Result<(), GatewayError> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::PairingCode) => {
            // Load config to get data_dir
            let config = Config::load()?;
            let socket_path = config.data_dir.join("pane-gateway.sock");

            // Connect to the running gateway's Unix socket
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            use tokio::net::UnixStream;

            let mut stream = match UnixStream::connect(&socket_path).await {
                Ok(s) => s,
                Err(_) => {
                    eprintln!("Error: Gateway is not running. Start it with: pane-gateway");
                    std::process::exit(1);
                }
            };

            // Send request
            let request = r#"{"command":"generate_pairing_code"}"#;
            stream
                .write_all(request.as_bytes())
                .await
                .map_err(GatewayError::Io)?;
            stream.shutdown().await.map_err(GatewayError::Io)?;

            // Read response
            let mut response = String::new();
            stream
                .read_to_string(&mut response)
                .await
                .map_err(GatewayError::Io)?;

            let parsed: serde_json::Value =
                serde_json::from_str(&response).map_err(|e| {
                    GatewayError::Config(format!("Invalid response from gateway: {}", e))
                })?;

            if let Some(code) = parsed["code"].as_str() {
                println!("{}", code);
            } else {
                eprintln!("Error: Unexpected response from gateway: {}", response);
                std::process::exit(1);
            }

            Ok(())
        }
        None => run_server().await,
    }
}

async fn run_server() -> Result<(), GatewayError> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    // Initialize tracing with JSON output
    tracing_subscriber::fmt()
        .json()
        .with_target(false)
        .with_timer(tracing_subscriber::fmt::time::UtcTime::rfc_3339())
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    eprintln!("=== Pane Gateway v{} ===", env!("CARGO_PKG_VERSION"));
    eprintln!();

    // Load configuration
    let config = Config::load()?;

    // Initialize auth state
    let tokens_path = config.data_dir.join("tokens.json");
    let auth_state = Arc::new(AuthState::new(tokens_path));

    // Generate pairing code.
    // #3106 M2: never print the code value to the process's own log stream —
    // zero characters, zero shape, same as every other pairing-code logging
    // site in this crate. Retrieve the code with `pane-gateway pairing-code`
    // (stdout, on explicit operator request over the IPC socket — not a log).
    let _pairing_code = auth_state.generate_pairing_code().await;
    eprintln!();
    eprintln!("=== Pairing Code Generated (valid for 5 minutes) ===");
    eprintln!("Run `pane-gateway pairing-code` to retrieve it.");
    eprintln!("======================================================");
    eprintln!();

    // Initialize OC client (optional - will connect lazily)
    let oc_url = std::env::var("OC_URL").unwrap_or_else(|_| "ws://localhost:5173".to_string());
    let oc_token =
        std::env::var("OC_BEARER_TOKEN").unwrap_or_else(|_| "test_token".to_string());

    // Derive HTTP base URL from WebSocket URL for REST API calls
    let oc_http_base_url = oc_url
        .replace("ws://", "http://")
        .replace("wss://", "https://");

    let oc_client = Arc::new(OcClient::new(oc_url, oc_token.clone())?);
    eprintln!("[MAIN] OC HTTP base URL: {}", oc_http_base_url);
    eprintln!("[MAIN] OC client initialized with device identity");

    // Initialize agent registry
    let agent_registry = Arc::new(AgentRegistry::new(oc_client.clone()));

    // §4A.2.2 warm-up: pre-populate the agent_registry cache for the default tenant so
    // the first live chat request doesn't block on a cold OC WebSocket RPC.
    // Failure is non-fatal — a cache miss on the first request will trigger the live RPC
    // transparently. Runs in background so it doesn't delay server startup.
    let registry_warmup = agent_registry.clone();
    tokio::spawn(async move {
        eprintln!("[MAIN] Warming up agent_registry cache for default tenant");
        match registry_warmup.get_agents("default", false).await {
            Ok(agents) => eprintln!(
                "[MAIN] agent_registry warm-up: cached {} agent(s) for default tenant",
                agents.len()
            ),
            Err(e) => eprintln!(
                "[MAIN] agent_registry warm-up: failed ({}); first chat will trigger live RPC",
                e
            ),
        }
    });

    // Load parameter compatibility table (strips deprecated params per downstream model — §4A.2.2)
    let param_compat_table = param_compat::CompatTable::load_or_default(&config.data_dir)?;

    // Load the remote-address allowlist checked at TCP-accept time (#3106 H1).
    // Config file, not a constant — see src-gateway/src/allowlist.rs.
    let allowlist = Allowlist::load_or_default(&config.data_dir)?;

    // Initialize metrics
    let metrics = Arc::new(Metrics::new());

    // Load or generate TLS certificate
    let tls_manager = TlsManager::new(
        &config.data_dir,
        config.tls_mode.clone(),
        config.tls_cert_path.clone(),
        config.tls_key_path.clone(),
    )?;
    let (cert_pem, key_pem, fingerprint, tls_hostname) = tls_manager.load_or_generate()?;

    eprintln!();
    eprintln!("=== TLS Certificate Fingerprint (SHA-256) ===");
    eprintln!("{}", fingerprint);
    eprintln!("===========================================");
    eprintln!();

    // Build TLS configuration
    let cert_pem_string = String::from_utf8(cert_pem.clone())
        .map_err(|e| GatewayError::Tls(format!("Invalid UTF-8 in certificate: {}", e)))?;
    let tls_config = build_tls_config(cert_pem, key_pem)?;

    // Start server
    start_server(
        config.port,
        tls_config,
        fingerprint,
        cert_pem_string,
        tls_hostname,
        auth_state,
        oc_client,
        agent_registry,
        metrics,
        oc_http_base_url,
        oc_token,
        config.data_dir.clone(),
        param_compat_table,
        config.streaming_idle_timeout_secs,
        allowlist,
    )
    .await?;

    Ok(())
}
