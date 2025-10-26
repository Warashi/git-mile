use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Direction {
    Client,
    Server,
}

#[derive(Debug, Deserialize)]
struct RecordedFrame {
    direction: Direction,
    payload: Value,
}

#[test]
fn reference_session_json_is_valid() {
    let frames: Vec<RecordedFrame> = serde_json::from_str(include_str!(
        "../../docs/reference/examples/mcp-session-list-show.json"
    ))
    .expect("reference session to be valid JSON");

    assert!(
        frames.len() >= 10,
        "expected at least 10 frames, found {}",
        frames.len()
    );

    let initialize = &frames[0];
    assert_eq!(initialize.direction, Direction::Client);
    assert_eq!(
        initialize.payload.get("method").and_then(Value::as_str),
        Some("initialize")
    );

    let initialize_result = &frames[1];
    assert_eq!(initialize_result.direction, Direction::Server);
    assert_eq!(
        initialize_result
            .payload
            .get("result")
            .and_then(|v| v.get("protocolVersion"))
            .and_then(Value::as_str),
        Some("2024-11-05")
    );

    let tools_response = &frames[3];
    let tools = tools_response
        .payload
        .get("result")
        .and_then(|r| r.get("tools"))
        .and_then(Value::as_array)
        .expect("tools/list response contains tools array");
    let mut tool_names = tools
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .collect::<Vec<_>>();
    tool_names.sort();
    assert_eq!(
        tool_names,
        vec!["git_mile.list", "git_mile.show"],
        "tool names should match exported MCP tools"
    );

    let list_message = &frames[5];
    let list_payload = list_message
        .payload
        .get("result")
        .and_then(|r| r.get("content"))
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|c| c.get("text"))
        .and_then(Value::as_str)
        .expect("list response contains text content");
    let list_json: Value =
        serde_json::from_str(list_payload).expect("list payload is valid JSON string");
    assert!(
        list_json.get("items").and_then(Value::as_array).is_some(),
        "list payload must include items array"
    );

    let show_message = &frames[7];
    let show_payload = show_message
        .payload
        .get("result")
        .and_then(|r| r.get("content"))
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|c| c.get("text"))
        .and_then(Value::as_str)
        .expect("show response contains text content");
    let show_json: Value =
        serde_json::from_str(show_payload).expect("show payload is valid JSON string");
    assert!(
        show_json.get("id").and_then(Value::as_str).is_some(),
        "show payload must include id"
    );
    assert!(
        show_json
            .get("comments")
            .and_then(Value::as_array)
            .is_some(),
        "show payload must include comments array"
    );

    let shutdown_message = &frames[9];
    assert_eq!(shutdown_message.direction, Direction::Server);
    assert!(shutdown_message
        .payload
        .get("result")
        .map(Value::is_null)
        .unwrap_or(false));
}
