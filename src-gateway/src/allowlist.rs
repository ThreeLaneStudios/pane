/// Remote-address allowlist, checked at TCP-accept time, before TLS (#3106 H1).
///
/// # Why this exists
///
/// `src-gateway` has no `SameMachine`/`SplitMachine`-conditional bind — that topology
/// branch does not exist in this crate and SameMachine was permanently retired
/// (#1108/#2649; see `docs/spec/sync.md` §22C.11). The server always binds
/// `0.0.0.0` (see `server.rs`) because SplitMachine, the only live topology, requires
/// the gateway reachable from a different machine. The control here is *who* may
/// complete a TCP connection, not *whether* the socket is reachable at all.
///
/// # Config file
///
/// The allowlist lives at `{data_dir}/allowlist.json` — a config file, not a
/// hardcoded constant, so an operator can see and override it without a rebuild.
/// On first run, if no file is found, the bundled default
/// (`src-gateway/config/allowlist-default.json`) is written to disk.
///
/// Default ranges: loopback (`127.0.0.1/32`, `::1/128`) plus the Tailscale CGNAT
/// range (`100.64.0.0/10`) and the Tailscale ULA range (`fd7a:115c:a1e0::/48`).
/// Loopback stays allowed by default — the gateway's own CLI (`pane-gateway
/// pairing-code`) and its tests talk to it locally on the OC machine.
///
/// Schema:
/// ```json
/// { "version": 1, "ranges": ["127.0.0.1/32", "100.64.0.0/10", "..."] }
/// ```
use crate::error::GatewayError;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;

/// Bundled default config; written to disk on first run if no config file exists.
const DEFAULT_CONFIG: &str = include_str!("../config/allowlist-default.json");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllowlistConfig {
    pub version: u32,
    pub ranges: Vec<String>,
}

/// A parsed CIDR range, normalized to a network address + prefix length per
/// address family. No third-party CIDR crate — the match set is small and fixed
/// shape (IPv4 and IPv6 only), so a hand-rolled mask keeps this dependency-free.
#[derive(Debug, Clone, PartialEq)]
enum CidrRange {
    V4 { network: u32, prefix_len: u32 },
    V6 { network: u128, prefix_len: u32 },
}

fn mask32(prefix_len: u32) -> u32 {
    if prefix_len == 0 {
        0
    } else {
        u32::MAX << (32 - prefix_len)
    }
}

fn mask128(prefix_len: u32) -> u128 {
    if prefix_len == 0 {
        0
    } else {
        u128::MAX << (128 - prefix_len)
    }
}

impl CidrRange {
    fn parse(entry: &str) -> Result<Self, String> {
        let (addr_part, prefix_part) = match entry.split_once('/') {
            Some((a, p)) => (a, Some(p)),
            None => (entry, None),
        };

        let addr: IpAddr = addr_part
            .parse()
            .map_err(|_| format!("invalid address in allowlist entry: {:?}", entry))?;

        match addr {
            IpAddr::V4(v4) => {
                let prefix_len: u32 = match prefix_part {
                    Some(p) => p
                        .parse()
                        .map_err(|_| format!("invalid IPv4 prefix length in allowlist entry: {:?}", entry))?,
                    None => 32,
                };
                if prefix_len > 32 {
                    return Err(format!("IPv4 prefix length out of range (0-32): {:?}", entry));
                }
                let network = u32::from(v4) & mask32(prefix_len);
                Ok(CidrRange::V4 { network, prefix_len })
            }
            IpAddr::V6(v6) => {
                let prefix_len: u32 = match prefix_part {
                    Some(p) => p
                        .parse()
                        .map_err(|_| format!("invalid IPv6 prefix length in allowlist entry: {:?}", entry))?,
                    None => 128,
                };
                if prefix_len > 128 {
                    return Err(format!("IPv6 prefix length out of range (0-128): {:?}", entry));
                }
                let network = u128::from(v6) & mask128(prefix_len);
                Ok(CidrRange::V6 { network, prefix_len })
            }
        }
    }

    fn contains(&self, addr: &IpAddr) -> bool {
        match (self, addr) {
            (CidrRange::V4 { network, prefix_len }, IpAddr::V4(v4)) => {
                (u32::from(*v4) & mask32(*prefix_len)) == *network
            }
            (CidrRange::V6 { network, prefix_len }, IpAddr::V6(v6)) => {
                (u128::from(*v6) & mask128(*prefix_len)) == *network
            }
            _ => false,
        }
    }
}

/// The parsed allowlist. Construct via [`Allowlist::load_or_default`] in
/// production, or [`Allowlist::from_ranges`] directly in tests.
pub struct Allowlist {
    ranges: Vec<CidrRange>,
}

impl Allowlist {
    /// Load from `data_dir/allowlist.json`.
    ///
    /// - File exists: parse it; return a clear error if malformed.
    /// - File missing: write the bundled default to disk, then use it.
    pub fn load_or_default(data_dir: &Path) -> Result<Arc<Self>, GatewayError> {
        let config_path = data_dir.join("allowlist.json");

        let contents = if config_path.exists() {
            eprintln!("[ALLOWLIST] Loading config from: {}", config_path.display());
            std::fs::read_to_string(&config_path).map_err(|e| {
                GatewayError::Config(format!(
                    "Failed to read allowlist.json at {}: {}",
                    config_path.display(),
                    e
                ))
            })?
        } else {
            eprintln!(
                "[ALLOWLIST] No config at {}; writing bundled default",
                config_path.display()
            );
            std::fs::create_dir_all(data_dir).map_err(|e| {
                GatewayError::Config(format!(
                    "Failed to create data directory {}: {}",
                    data_dir.display(),
                    e
                ))
            })?;
            std::fs::write(&config_path, DEFAULT_CONFIG).map_err(|e| {
                GatewayError::Config(format!(
                    "Failed to write default allowlist.json to {}: {}",
                    config_path.display(),
                    e
                ))
            })?;
            DEFAULT_CONFIG.to_string()
        };

        let parsed: AllowlistConfig = serde_json::from_str(&contents).map_err(|e| {
            GatewayError::Config(format!(
                "Malformed allowlist.json at {}: {}",
                config_path.display(),
                e
            ))
        })?;

        let allowlist = Self::from_ranges(&parsed.ranges).map_err(GatewayError::Config)?;
        eprintln!("[ALLOWLIST] Loaded {} range(s) (version {})", parsed.ranges.len(), parsed.version);
        Ok(Arc::new(allowlist))
    }

    /// Build directly from a list of CIDR strings — used by `load_or_default`
    /// and by tests that want a fixture allowlist without touching disk.
    pub fn from_ranges(entries: &[String]) -> Result<Self, String> {
        let mut ranges = Vec::with_capacity(entries.len());
        for entry in entries {
            ranges.push(CidrRange::parse(entry)?);
        }
        Ok(Self { ranges })
    }

    /// Is `addr` permitted to complete a connection?
    ///
    /// IPv4-mapped IPv6 addresses (`::ffff:a.b.c.d`) are unwrapped to their IPv4
    /// form first, so a dual-stack accept doesn't bypass an IPv4-only range.
    pub fn is_allowed(&self, addr: IpAddr) -> bool {
        let addr = match addr {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v6)),
            other => other,
        };
        self.ranges.iter().any(|r| r.contains(&addr))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn default_allowlist() -> Allowlist {
        let config: AllowlistConfig = serde_json::from_str(DEFAULT_CONFIG).unwrap();
        Allowlist::from_ranges(&config.ranges).unwrap()
    }

    #[test]
    fn test_bundled_default_parses() {
        // The file this test reads is the exact file shipped to operators —
        // if it doesn't parse, nothing in this module works.
        let _ = default_allowlist();
    }

    #[test]
    fn test_loopback_v4_allowed_by_default() {
        let allowlist = default_allowlist();
        assert!(allowlist.is_allowed(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
    }

    #[test]
    fn test_loopback_v6_allowed_by_default() {
        let allowlist = default_allowlist();
        assert!(allowlist.is_allowed(IpAddr::V6(Ipv6Addr::LOCALHOST)));
    }

    #[test]
    fn test_tailscale_cgnat_range_allowed_by_default() {
        let allowlist = default_allowlist();
        // 100.64.0.0/10 spans 100.64.0.0 - 100.127.255.255
        assert!(allowlist.is_allowed(IpAddr::V4(Ipv4Addr::new(100, 100, 50, 7))));
        assert!(allowlist.is_allowed(IpAddr::V4(Ipv4Addr::new(100, 64, 0, 0))));
        assert!(allowlist.is_allowed(IpAddr::V4(Ipv4Addr::new(100, 127, 255, 255))));
    }

    #[test]
    fn test_tailscale_ula_range_allowed_by_default() {
        let allowlist = default_allowlist();
        let addr: Ipv6Addr = "fd7a:115c:a1e0:1234:5678::1".parse().unwrap();
        assert!(allowlist.is_allowed(IpAddr::V6(addr)));
    }

    /// The literal red case Matt named: "a connection from a non-allowlisted
    /// address is accepted today and refused after." A public internet address
    /// — well outside loopback, the Tailscale CGNAT range, and the Tailscale ULA
    /// range — must be rejected under the default config.
    #[test]
    fn test_arbitrary_public_address_rejected_by_default() {
        let allowlist = default_allowlist();
        assert!(!allowlist.is_allowed(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
    }

    #[test]
    fn test_adjacent_cgnat_boundary_rejected() {
        let allowlist = default_allowlist();
        // 100.64.0.0/10 ends at 100.127.255.255 — one past that is outside the range.
        assert!(!allowlist.is_allowed(IpAddr::V4(Ipv4Addr::new(100, 128, 0, 0))));
        // One before the range starts.
        assert!(!allowlist.is_allowed(IpAddr::V4(Ipv4Addr::new(100, 63, 255, 255))));
    }

    #[test]
    fn test_other_private_ranges_not_allowed_by_default() {
        let allowlist = default_allowlist();
        // RFC1918 LAN ranges are NOT in the default allowlist — only Tailscale +
        // loopback are. A plain home-LAN peer that isn't on the tailnet is rejected.
        assert!(!allowlist.is_allowed(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50))));
        assert!(!allowlist.is_allowed(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5))));
    }

    #[test]
    fn test_ipv4_mapped_ipv6_normalized_before_check() {
        let allowlist = default_allowlist();
        let mapped: Ipv6Addr = "::ffff:100.100.50.7".parse().unwrap();
        assert!(allowlist.is_allowed(IpAddr::V6(mapped)));
        let mapped_public: Ipv6Addr = "::ffff:8.8.8.8".parse().unwrap();
        assert!(!allowlist.is_allowed(IpAddr::V6(mapped_public)));
    }

    #[test]
    fn test_from_ranges_rejects_malformed_entry() {
        let result = Allowlist::from_ranges(&["not-an-address".to_string()]);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_ranges_rejects_out_of_range_prefix() {
        let result = Allowlist::from_ranges(&["10.0.0.0/33".to_string()]);
        assert!(result.is_err());
    }

    #[test]
    fn test_custom_allowlist_can_narrow_or_widen() {
        // Operator-overridable: a custom allowlist can be strictly narrower
        // (no Tailscale, loopback only) or wider (adds a specific LAN host)
        // than the bundled default — proving this isn't a hardcoded constant.
        let narrow = Allowlist::from_ranges(&["127.0.0.1/32".to_string()]).unwrap();
        assert!(narrow.is_allowed(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
        assert!(!narrow.is_allowed(IpAddr::V4(Ipv4Addr::new(100, 100, 50, 7))));

        let widened = Allowlist::from_ranges(&[
            "127.0.0.1/32".to_string(),
            "192.168.1.100/32".to_string(),
        ])
        .unwrap();
        assert!(widened.is_allowed(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100))));
        assert!(!widened.is_allowed(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 101))));
    }
}
