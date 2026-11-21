use crate::error::GatewayError;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TlsMode {
    Auto,       // Try Tailscale → fallback to self-signed (disabled in V1)
    Tailscale,  // Tailscale only, error if unavailable (disabled in V1)
    #[default]
    SelfSigned, // Current behavior — default in V1
    Custom,     // Load from tls_cert_path + tls_key_path
}

impl std::str::FromStr for TlsMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "auto" => Ok(TlsMode::Auto),
            "tailscale" => Ok(TlsMode::Tailscale),
            "selfsigned" => Ok(TlsMode::SelfSigned),
            "custom" => Ok(TlsMode::Custom),
            _ => Err(format!("Invalid TLS mode: {}", s)),
        }
    }
}

/// Gateway configuration
#[derive(Debug, Clone)]
pub struct Config {
    /// Port to listen on
    pub port: u16,
    /// Data directory for certs and state
    pub data_dir: PathBuf,
    /// OpenClaw URL (for future use)
    pub openclaw_url: Option<String>,
    /// TLS mode selection
    pub tls_mode: TlsMode,
    /// Custom TLS certificate path
    pub tls_cert_path: Option<PathBuf>,
    /// Custom TLS private key path
    pub tls_key_path: Option<PathBuf>,
    /// Idle timeout for streaming responses: drop the connection only if NO chunk
    /// arrives for this many seconds. As long as bytes keep flowing, the stream
    /// lives. For non-streaming requests the existing 60s total timeout applies.
    /// Configurable via GATEWAY_STREAMING_IDLE_TIMEOUT_SECS (default: 60).
    pub streaming_idle_timeout_secs: u64,
}

impl Config {
    /// Load configuration from environment variables with defaults
    pub fn load() -> Result<Self, GatewayError> {
        tracing::info!(component = "config", "Loading gateway configuration");
        
        let port = std::env::var("GATEWAY_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(8443);

        let data_dir = std::env::var("GATEWAY_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME")
                    .expect("HOME environment variable not set");
                PathBuf::from(home).join(".pane-gateway")
            });

        let openclaw_url = std::env::var("OPENCLAW_URL").ok();

        let tls_mode = std::env::var("GATEWAY_TLS_MODE")
            .ok()
            .and_then(|m| m.parse().ok())
            .unwrap_or(TlsMode::SelfSigned);

        // Hard-gate: Tailscale and Auto TLS modes are not available in V1.
        // The SPKI pin model is incompatible with LE cert rotation (new key per 90-day renewal).
        // See: docs/audits/2026-07-12-2011-final-implementation-scope.md, Scope Item 2.
        if matches!(tls_mode, TlsMode::Tailscale | TlsMode::Auto) {
            eprintln!(
                "[FATAL] TLS mode '{:?}' is not available in V1. \
                 Remove GATEWAY_TLS_MODE or set it to 'selfsigned'.",
                tls_mode
            );
            std::process::exit(1);
        }

        let tls_cert_path = std::env::var("GATEWAY_TLS_CERT_PATH")
            .ok()
            .map(PathBuf::from);

        let tls_key_path = std::env::var("GATEWAY_TLS_KEY_PATH")
            .ok()
            .map(PathBuf::from);

        tracing::info!(component = "config", port = port, data_dir = %data_dir.display(), "Configuration loaded");
        tracing::info!(component = "config", tls_mode = ?tls_mode, "TLS mode set");
        if let Some(ref url) = openclaw_url {
            tracing::info!(component = "config", url = %url, "OpenClaw URL configured");
        }
        if let Some(ref path) = tls_cert_path {
            tracing::info!(component = "config", path = %path.display(), "Custom TLS cert path set");
        }
        if let Some(ref path) = tls_key_path {
            tracing::info!(component = "config", path = %path.display(), "Custom TLS key path set");
        }

        let streaming_idle_timeout_secs = std::env::var("GATEWAY_STREAMING_IDLE_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(60);

        tracing::info!(
            component = "config",
            streaming_idle_timeout_secs = streaming_idle_timeout_secs,
            "Streaming idle timeout configured"
        );

        Ok(Config {
            port,
            data_dir,
            openclaw_url,
            tls_mode,
            tls_cert_path,
            tls_key_path,
            streaming_idle_timeout_secs,
        })
    }
}
