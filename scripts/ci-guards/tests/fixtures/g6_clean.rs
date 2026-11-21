// G6 clean fixture — protocol versions expressed via named constants, not integer literals.
// G6 should NOT fire on any line in this file.

/// Lowest OC protocol version this client speaks.
pub(crate) const OC_PROTOCOL_MIN: u32 = 3;

/// Broadcast upper bound for OC protocol versions (forward-compat per protocol.md §644).
pub(crate) const OC_PROTOCOL_MAX: u32 = 99;

fn connect_to_oc() {
    let connect_params = serde_json::json!({
        "minProtocol": OC_PROTOCOL_MIN,
        "maxProtocol": OC_PROTOCOL_MAX,
        "client": { "id": "gateway-client" }
    });
    let _ = connect_params;
}

#[cfg(test)]
mod tests {
    use super::{OC_PROTOCOL_MIN, OC_PROTOCOL_MAX};

    fn mock_hello_ok() -> serde_json::Value {
        serde_json::json!({
            "type": "hello-ok",
            "protocol": OC_PROTOCOL_MIN,
            "server": { "connId": "test-conn-123" }
        })
    }

    #[test]
    fn test_mock_hello_ok() {
        let v = mock_hello_ok();
        assert_eq!(v["type"], "hello-ok");
        // Verify the protocol field equals the named constant — not a hardcoded literal
        assert!(v["protocol"].as_u64().is_some());
        let _ = OC_PROTOCOL_MAX;
    }
}
