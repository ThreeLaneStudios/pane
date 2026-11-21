/// Audit marker prefixes for daily log entries (§5.6.3A)
/// Used by both the write side (extraction, Memory Settings) and the filter side (extraction pipeline)
pub const MARKER_SAVED: &str = "[saved]";
pub const MARKER_EDITED: &str = "[memory edited]";
pub const MARKER_DELETED: &str = "[memory deleted]";

/// Check if a line contains any audit marker.
/// Uses simple contains() for robustness — no position-dependent parsing.
pub fn is_audit_marker(line: &str) -> bool {
    line.contains(MARKER_SAVED)
        || line.contains(MARKER_EDITED)
        || line.contains(MARKER_DELETED)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_audit_marker_saved() {
        assert!(is_audit_marker("- 14:30 [saved] fact: \"some fact\""));
        assert!(is_audit_marker("[saved] fact: \"test\""));
    }

    #[test]
    fn test_is_audit_marker_edited() {
        assert!(is_audit_marker("- 09:15 [memory edited] preference: \"old\" → \"new\""));
    }

    #[test]
    fn test_is_audit_marker_deleted() {
        assert!(is_audit_marker("- 22:00 [memory deleted] fact: \"removed\" (user removed via Memory Settings)"));
    }

    #[test]
    fn test_normal_content_not_marker() {
        assert!(!is_audit_marker("- 14:30 User mentioned they like coffee"));
        assert!(!is_audit_marker("Regular conversation text"));
        assert!(!is_audit_marker(""));
    }
}
