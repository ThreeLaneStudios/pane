use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

const SERVICE_TYPE: &str = "_pane-sync._tcp.local.";
const PANE_SYNC_PORT: u16 = 21059;

pub struct PeerDiscovery {
    daemon: ServiceDaemon,
    peers: Arc<Mutex<HashSet<String>>>,
    instance_name: String,
}

impl PeerDiscovery {
    pub fn new(instance_name: String) -> Result<Self, String> {
        let daemon = ServiceDaemon::new()
            .map_err(|e| format!("Failed to create mDNS daemon: {}", e))?;

        Ok(Self {
            daemon,
            peers: Arc::new(Mutex::new(HashSet::new())),
            instance_name,
        })
    }

    pub fn register_service(&self) -> Result<(), String> {
        let hostname = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "pane-host".to_string());

        let service_info = ServiceInfo::new(
            SERVICE_TYPE,
            &self.instance_name,
            &format!("{}.", hostname),
            "",
            PANE_SYNC_PORT,
            None,
        ).map_err(|e| format!("Failed to create service info: {}", e))?;

        self.daemon.register(service_info)
            .map_err(|e| format!("Failed to register service: {}", e))?;

        log::info!("Registered mDNS service: {} on port {}", self.instance_name, PANE_SYNC_PORT);
        Ok(())
    }

    pub fn start_browsing(&self) -> Result<(), String> {
        let receiver = self.daemon.browse(SERVICE_TYPE)
            .map_err(|e| format!("Failed to browse for services: {}", e))?;

        let peers = self.peers.clone();
        let my_name = self.instance_name.clone();

        std::thread::spawn(move || {
            while let Ok(event) = receiver.recv() {
                match event {
                    ServiceEvent::ServiceResolved(info) => {
                        if info.get_fullname().contains(&my_name) {
                            continue; // Skip self
                        }
                        let addresses = info.get_addresses();
                        for addr in addresses {
                            let peer = format!("{}:{}", addr, info.get_port());
                            log::info!("Discovered Pane peer: {}", peer);
                            if let Ok(mut p) = peers.lock() {
                                p.insert(peer);
                            }
                        }
                    }
                    ServiceEvent::ServiceRemoved(_, fullname) => {
                        log::info!("Pane peer removed: {}", fullname);
                        // We don't have enough info to remove specific addr, but that's ok
                    }
                    _ => {}
                }
            }
        });

        Ok(())
    }

    pub fn get_peers(&self) -> Vec<String> {
        self.peers.lock()
            .map(|p| p.iter().cloned().collect())
            .unwrap_or_default()
    }
}
