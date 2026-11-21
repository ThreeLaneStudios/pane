use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct Metrics {
    pub requests_total: Arc<AtomicU64>,
    pub requests_by_endpoint: Arc<RwLock<HashMap<String, AtomicU64>>>,
    pub errors_by_status: Arc<RwLock<HashMap<u16, AtomicU64>>>,
    pub oc_method_calls: Arc<RwLock<HashMap<String, AtomicU64>>>,
    pub sync_queue_depth: Arc<AtomicU64>,
    pub ws_reconnects: Arc<AtomicU64>,
    pub sse_replays: Arc<AtomicU64>,
    pub sse_replay_gaps: Arc<AtomicU64>,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            requests_total: Arc::new(AtomicU64::new(0)),
            requests_by_endpoint: Arc::new(RwLock::new(HashMap::new())),
            errors_by_status: Arc::new(RwLock::new(HashMap::new())),
            oc_method_calls: Arc::new(RwLock::new(HashMap::new())),
            sync_queue_depth: Arc::new(AtomicU64::new(0)),
            ws_reconnects: Arc::new(AtomicU64::new(0)),
            sse_replays: Arc::new(AtomicU64::new(0)),
            sse_replay_gaps: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn inc_requests_total(&self) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_requests_by_endpoint(&self, endpoint: &str) {
        let mut map = self.requests_by_endpoint.write();
        map.entry(endpoint.to_string())
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_errors_by_status(&self, status: u16) {
        let mut map = self.errors_by_status.write();
        map.entry(status)
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_oc_method_calls(&self, method: &str) {
        let mut map = self.oc_method_calls.write();
        map.entry(method.to_string())
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn set_sync_queue_depth(&self, depth: u64) {
        self.sync_queue_depth.store(depth, Ordering::Relaxed);
    }

    pub fn inc_ws_reconnects(&self) {
        self.ws_reconnects.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_sse_replays(&self) {
        self.sse_replays.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_sse_replay_gaps(&self) {
        self.sse_replay_gaps.fetch_add(1, Ordering::Relaxed);
    }

    pub fn export_prometheus(&self) -> String {
        let mut output = String::new();

        output.push_str("# HELP gateway_requests_total Total requests received\n");
        output.push_str("# TYPE gateway_requests_total counter\n");
        output.push_str(&format!(
            "gateway_requests_total {}\n\n",
            self.requests_total.load(Ordering::Relaxed)
        ));

        output.push_str("# HELP gateway_requests_by_endpoint Requests by endpoint\n");
        output.push_str("# TYPE gateway_requests_by_endpoint counter\n");
        {
            let map = self.requests_by_endpoint.read();
            for (endpoint, counter) in map.iter() {
                output.push_str(&format!(
                    "gateway_requests_by_endpoint{{endpoint=\"{}\"}} {}\n",
                    endpoint,
                    counter.load(Ordering::Relaxed)
                ));
            }
        }
        output.push_str("\n");

        output.push_str("# HELP gateway_errors_by_status Errors by HTTP status\n");
        output.push_str("# TYPE gateway_errors_by_status counter\n");
        {
            let map = self.errors_by_status.read();
            for (status, counter) in map.iter() {
                output.push_str(&format!(
                    "gateway_errors_by_status{{status=\"{}\"}} {}\n",
                    status,
                    counter.load(Ordering::Relaxed)
                ));
            }
        }
        output.push_str("\n");

        output.push_str("# HELP gateway_oc_method_calls OC WebSocket method calls\n");
        output.push_str("# TYPE gateway_oc_method_calls counter\n");
        {
            let map = self.oc_method_calls.read();
            for (method, counter) in map.iter() {
                output.push_str(&format!(
                    "gateway_oc_method_calls{{method=\"{}\"}} {}\n",
                    method,
                    counter.load(Ordering::Relaxed)
                ));
            }
        }
        output.push_str("\n");

        output.push_str("# HELP gateway_sync_queue_depth Current sync queue depth\n");
        output.push_str("# TYPE gateway_sync_queue_depth gauge\n");
        output.push_str(&format!(
            "gateway_sync_queue_depth {}\n\n",
            self.sync_queue_depth.load(Ordering::Relaxed)
        ));

        output.push_str("# HELP gateway_ws_reconnects WebSocket reconnection count\n");
        output.push_str("# TYPE gateway_ws_reconnects counter\n");
        output.push_str(&format!(
            "gateway_ws_reconnects {}\n\n",
            self.ws_reconnects.load(Ordering::Relaxed)
        ));

        output.push_str("# HELP gateway_sse_replays SSE event replays via Last-Event-ID\n");
        output.push_str("# TYPE gateway_sse_replays counter\n");
        output.push_str(&format!(
            "gateway_sse_replays {}\n\n",
            self.sse_replays.load(Ordering::Relaxed)
        ));

        output.push_str("# HELP gateway_sse_replay_gaps SSE replay gaps (events lost)\n");
        output.push_str("# TYPE gateway_sse_replay_gaps counter\n");
        output.push_str(&format!(
            "gateway_sse_replay_gaps {}",
            self.sse_replay_gaps.load(Ordering::Relaxed)
        ));

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_increment_on_request() {
        let metrics = Metrics::new();
        assert_eq!(metrics.requests_total.load(Ordering::Relaxed), 0);
        metrics.inc_requests_total();
        metrics.inc_requests_by_endpoint("/v1/sync/push");
        assert_eq!(metrics.requests_total.load(Ordering::Relaxed), 1);
        let map = metrics.requests_by_endpoint.read();
        assert_eq!(map.get("/v1/sync/push").unwrap().load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_prometheus_format() {
        let metrics = Metrics::new();
        metrics.inc_requests_total();
        metrics.inc_requests_by_endpoint("/v1/health");
        metrics.inc_errors_by_status(401);
        metrics.inc_oc_method_calls("sessions.create");
        metrics.set_sync_queue_depth(5);
        let output = metrics.export_prometheus();
        assert!(output.contains("# HELP gateway_requests_total"));
        assert!(output.contains("gateway_requests_total 1"));
        assert!(output.contains("gateway_requests_by_endpoint{endpoint=\"/v1/health\"} 1"));
        assert!(output.contains("gateway_errors_by_status{status=\"401\"} 1"));
        assert!(output.contains("gateway_oc_method_calls{method=\"sessions.create\"} 1"));
        assert!(output.contains("gateway_sync_queue_depth 5"));
    }
}
