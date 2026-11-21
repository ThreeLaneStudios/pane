use std::sync::atomic::{AtomicBool, Ordering};

pub struct AutoSync {
    enabled: AtomicBool,
    push_pending: AtomicBool,
}

impl AutoSync {
    pub fn new() -> Self {
        AutoSync {
            enabled: AtomicBool::new(false),
            push_pending: AtomicBool::new(false),
        }
    }
    
    pub fn enable(&self) {
        self.enabled.store(true, Ordering::SeqCst);
    }
    
    pub fn disable(&self) {
        self.enabled.store(false, Ordering::SeqCst);
    }
    
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }
    
    /// Mark that a push is needed (called after DB writes)
    pub fn notify_change(&self) {
        if self.is_enabled() {
            self.push_pending.store(true, Ordering::SeqCst);
        }
    }
    
    /// Check and clear the pending flag
    // Retained for the dormant automatic push scheduler.
    #[allow(dead_code)]
    pub fn take_pending(&self) -> bool {
        self.push_pending.swap(false, Ordering::SeqCst)
    }
}
