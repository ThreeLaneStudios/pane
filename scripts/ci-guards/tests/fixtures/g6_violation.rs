// G6 violation fixture — hardcoded protocol integer literals in connect params.
// G6 should flag all three lines below.
fn connect_to_oc() {
    let connect_params = serde_json::json!({
        "minProtocol": 3,
        "maxProtocol": 3,
        "client": { "id": "gateway-client" }
    });
    let _ = connect_params;
}

#[cfg(test)]
mod tests {
    // G6 scope includes test modules — this is intentional per #1336.
    // A test that hardcodes "protocol": 4 is a blind spot.
    fn mock_hello_ok() -> serde_json::Value {
        serde_json::json!({
            "type": "hello-ok",
            "protocol": 4,
            "server": { "connId": "test-conn-123" }
        })
    }

    #[test]
    fn test_mock_hello_ok() {
        let v = mock_hello_ok();
        assert_eq!(v["type"], "hello-ok");
    }
}
