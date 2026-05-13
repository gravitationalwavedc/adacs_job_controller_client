use crate::resolve_websocket_token;
use serde_json::json;

#[test]
fn resolve_websocket_token_prefers_config_ltk() {
    let args = vec!["client".to_string(), "/tmp/libpython.so".to_string()];
    let result = resolve_websocket_token(&args, &json!({"ltk": "persisted-token"})).unwrap();

    assert_eq!(result, ("persisted-token".to_string(), true));
}

#[test]
fn resolve_websocket_token_uses_cli_when_no_ltk() {
    let args = vec![
        "client".to_string(),
        "/tmp/libpython.so".to_string(),
        "cli-token".to_string(),
    ];
    let result = resolve_websocket_token(&args, &json!({})).unwrap();

    assert_eq!(result, ("cli-token".to_string(), false));
}

#[test]
fn resolve_websocket_token_rejects_both_sources() {
    let args = vec![
        "client".to_string(),
        "/tmp/libpython.so".to_string(),
        "cli-token".to_string(),
    ];
    let err = resolve_websocket_token(&args, &json!({"ltk": "persisted-token"})).unwrap_err();

    assert!(err.contains("Both config.json 'ltk' and a CLI WebSocket token"));
}

#[test]
fn resolve_websocket_token_requires_one_source() {
    let args = vec!["client".to_string(), "/tmp/libpython.so".to_string()];
    let err = resolve_websocket_token(&args, &json!({"ltk": "   "})).unwrap_err();

    assert!(err.contains("No WebSocket token configured"));
}
