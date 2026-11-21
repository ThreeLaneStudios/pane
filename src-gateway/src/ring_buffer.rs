use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Maximum events per session buffer
const MAX_EVENTS: usize = 1000;

/// Maximum age of events in buffer (5 minutes)
const MAX_AGE_SECONDS: i64 = 300;

/// A buffered event with sequence number and timestamp
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BufferedEvent {
    pub sequence: u64,
    pub event_type: String,
    pub data: serde_json::Value,
    pub timestamp: DateTime<Utc>,
}

/// Per-session ring buffer for event replay
/// Stores last 1000 events OR last 5 minutes (whichever fills first)
pub struct RingBuffer {
    /// Buffers keyed by session_id
    buffers: Arc<RwLock<HashMap<String, SessionBuffer>>>,
}

/// Single session's ring buffer
#[derive(Debug)]
struct SessionBuffer {
    events: VecDeque<BufferedEvent>,
    next_sequence: u64,
}

impl SessionBuffer {
    fn new() -> Self {
        Self {
            events: VecDeque::with_capacity(MAX_EVENTS),
            next_sequence: 1,
        }
    }

    /// Add event and evict oldest if needed
    fn push(&mut self, event_type: String, data: serde_json::Value) -> BufferedEvent {
        let event = BufferedEvent {
            sequence: self.next_sequence,
            event_type,
            data,
            timestamp: Utc::now(),
        };

        self.next_sequence += 1;

        // Add to buffer
        self.events.push_back(event.clone());

        // Evict by count (keep last MAX_EVENTS)
        while self.events.len() > MAX_EVENTS {
            self.events.pop_front();
        }

        // Evict by time (keep last MAX_AGE_SECONDS)
        let now = Utc::now();
        let cutoff = now - Duration::seconds(MAX_AGE_SECONDS);
        while let Some(oldest) = self.events.front() {
            if oldest.timestamp < cutoff {
                self.events.pop_front();
            } else {
                break;
            }
        }

        event
    }

    /// Get all events after a given sequence number
    fn replay_from(&self, after_seq: u64) -> ReplayResult {
        // Check if requested sequence is before our oldest event
        if let Some(oldest) = self.events.front() {
            // Special case: after_seq of 0 means "from the beginning"
            // If oldest is > 1, it means events 1 through oldest-1 were evicted
            if after_seq == 0 && oldest.sequence > 1 {
                return ReplayResult::Gap {
                    oldest_available: oldest.sequence,
                };
            }
            
            // General case: if requested seq is before oldest (and not 0), gap
            if after_seq > 0 && after_seq < oldest.sequence {
                return ReplayResult::Gap {
                    oldest_available: oldest.sequence,
                };
            }
        }

        // Collect events after requested sequence
        let events: Vec<BufferedEvent> = self
            .events
            .iter()
            .filter(|e| e.sequence > after_seq)
            .cloned()
            .collect();

        ReplayResult::Events(events)
    }
}

/// Result of a replay request
#[derive(Debug)]
pub enum ReplayResult {
    /// Successfully retrieved events
    Events(Vec<BufferedEvent>),
    /// Gap detected - requested sequence too old
    Gap { oldest_available: u64 },
}

impl RingBuffer {
    pub fn new() -> Self {
        eprintln!("[RING_BUFFER] Creating ring buffer manager");
        Self {
            buffers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Push an event to a session's ring buffer
    pub async fn push(
        &self,
        session_id: &str,
        event_type: String,
        data: serde_json::Value,
    ) -> BufferedEvent {
        eprintln!(
            "[RING_BUFFER] Pushing event: session={}, type={}",
            session_id, event_type
        );

        let mut buffers = self.buffers.write().await;
        let buffer = buffers
            .entry(session_id.to_string())
            .or_insert_with(SessionBuffer::new);

        buffer.push(event_type, data)
    }

    /// Replay events from a given sequence number
    pub async fn replay_from(&self, session_id: &str, after_seq: u64) -> ReplayResult {
        eprintln!(
            "[RING_BUFFER] Replaying: session={}, after_seq={}",
            session_id, after_seq
        );

        let buffers = self.buffers.read().await;
        match buffers.get(session_id) {
            Some(buffer) => buffer.replay_from(after_seq),
            None => {
                eprintln!("[RING_BUFFER] No buffer found for session");
                ReplayResult::Events(vec![])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_push_and_replay() {
        let buffer = RingBuffer::new();
        let session_id = "test-session";

        // Push some events
        let event1 = buffer
            .push(session_id, "message.delta".to_string(), json!({"text": "hello"}))
            .await;

        let event2 = buffer
            .push(
                session_id,
                "message.delta".to_string(),
                json!({"text": " world"}),
            )
            .await;

        // Replay from sequence 0 (get all events)
        let result = buffer.replay_from(session_id, 0).await;
        match result {
            ReplayResult::Events(events) => {
                assert_eq!(events.len(), 2);
                assert_eq!(events[0].sequence, event1.sequence);
                assert_eq!(events[1].sequence, event2.sequence);
            }
            ReplayResult::Gap { .. } => panic!("Expected events, got gap"),
        }

        // Replay from event1 (get only event2)
        let result = buffer.replay_from(session_id, event1.sequence).await;
        match result {
            ReplayResult::Events(events) => {
                assert_eq!(events.len(), 1);
                assert_eq!(events[0].sequence, event2.sequence);
            }
            ReplayResult::Gap { .. } => panic!("Expected events, got gap"),
        }
    }

    #[tokio::test]
    async fn test_count_eviction() {
        let buffer = RingBuffer::new();
        let session_id = "test-session";

        // Push MAX_EVENTS + 10 events
        for i in 0..(MAX_EVENTS + 10) {
            buffer
                .push(
                    session_id,
                    "message.delta".to_string(),
                    json!({"index": i}),
                )
                .await;
        }

        // Replay from 0 should detect a gap (first 10 events evicted)
        let result = buffer.replay_from(session_id, 0).await;
        match result {
            ReplayResult::Gap { oldest_available } => {
                // Oldest available should be event 11 (1-indexed, evicted 1-10)
                assert_eq!(oldest_available, 11);
            }
            ReplayResult::Events(_) => panic!("Expected gap, got events"),
        }

        // Replay from event 15 should work
        let result = buffer.replay_from(session_id, 15).await;
        match result {
            ReplayResult::Events(events) => {
                // Should get events 16 through (MAX_EVENTS + 10)
                assert_eq!(events.len(), (MAX_EVENTS + 10) - 15);
                assert_eq!(events[0].sequence, 16);
            }
            ReplayResult::Gap { .. } => panic!("Expected events, got gap"),
        }
    }

    #[tokio::test]
    async fn test_multiple_sessions() {
        let buffer = RingBuffer::new();

        // Push events to two different sessions
        buffer
            .push("session1", "test".to_string(), json!({"msg": "a"}))
            .await;

        buffer
            .push("session2", "test".to_string(), json!({"msg": "b"}))
            .await;

        buffer
            .push("session1", "test".to_string(), json!({"msg": "c"}))
            .await;

        // Each session should have independent buffers
        let result1 = buffer.replay_from("session1", 0).await;
        let result2 = buffer.replay_from("session2", 0).await;

        match (result1, result2) {
            (ReplayResult::Events(events1), ReplayResult::Events(events2)) => {
                assert_eq!(events1.len(), 2); // session1 has 2 events
                assert_eq!(events2.len(), 1); // session2 has 1 event
            }
            _ => panic!("Expected events for both sessions"),
        }
    }

    #[tokio::test]
    async fn test_empty_session_replay() {
        let buffer = RingBuffer::new();

        // Replay from a session that doesn't exist
        let result = buffer.replay_from("nonexistent", 0).await;
        match result {
            ReplayResult::Events(events) => {
                assert_eq!(events.len(), 0);
            }
            ReplayResult::Gap { .. } => panic!("Expected empty events, got gap"),
        }
    }
}
