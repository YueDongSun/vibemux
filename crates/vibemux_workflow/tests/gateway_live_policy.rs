use serde_json::json;
use vibemux_workflow::gateway::GatewayConfig;

fn legacy_config() -> serde_json::Value {
    json!({"schema_version":1,"endpoint":"http://127.0.0.1:18766","credential_env":"AAG_POLICY_FIXTURE_TOKEN","bindings":[],"strict_routing":true})
}

#[test]
fn explicit_bound_receipt_policy_is_recognized_without_changing_legacy_serialization() {
    let legacy = legacy_config();
    let parsed = GatewayConfig::parse(&serde_json::to_vec(&legacy).unwrap()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), legacy);
    let mut enabled = legacy;
    enabled["live_admission"] = json!("bound_receipt_v2");
    let parsed = GatewayConfig::parse(&serde_json::to_vec(&enabled).unwrap())
        .expect("owner can explicitly require bound v2 receipts");
    assert_eq!(serde_json::to_value(parsed).unwrap(), enabled);
}

#[test]
fn future_or_permissive_policy_values_are_rejected() {
    for value in [
        json!("best_effort"),
        json!("bound_receipt_v1"),
        json!(true),
        json!(null),
    ] {
        let mut config = legacy_config();
        config["live_admission"] = value;
        assert!(GatewayConfig::parse(&serde_json::to_vec(&config).unwrap()).is_err());
    }
}
