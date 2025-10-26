use anyhow::{anyhow, Result};
use git_mile_e2e::{create_repository_fixture, McpHarness, Response};
use serde_json::{json, Value};

fn extract_content(result: Value) -> Result<Value> {
    let content = result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .ok_or_else(|| anyhow!("missing content array"))?;
    let text = content
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("content missing text field"))?;
    let parsed: Value = serde_json::from_str(text)?;
    Ok(parsed)
}

#[test]
fn list_flow_returns_fixture_data() -> Result<()> {
    let repo = create_repository_fixture()?;
    let mut harness = McpHarness::spawn(repo.path())?;

    match harness.initialize()? {
        Response::Result(_) => {}
        Response::Error(err) => return Err(anyhow!("initialize failed: {err:?}")),
    }

    match harness.list_tools()? {
        Response::Result(tools) => {
            let names = tools
                .get("tools")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow!("tools/list missing tools array"))?
                .iter()
                .filter_map(|tool| tool.get("name").and_then(Value::as_str))
                .collect::<Vec<_>>();
            assert!(
                names.contains(&"git_mile.list") && names.contains(&"git_mile.show"),
                "unexpected tool registry: {names:?}"
            );
        }
        Response::Error(err) => return Err(anyhow!("tools/list failed: {err:?}")),
    }

    let response = harness.call_tool(
        "git_mile.list",
        json!({
            "entity": "milestone",
            "includeClosed": true,
            "limit": 10
        }),
    )?;

    let payload = match response {
        Response::Result(value) => value,
        Response::Error(err) => return Err(anyhow!("git_mile.list returned error: {err:?}")),
    };

    let list_json = extract_content(payload)?;
    let items = list_json
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("list payload missing items"))?;
    assert!(
        !items.is_empty(),
        "expected at least one milestone, got {items:?}"
    );
    let ids = items
        .iter()
        .filter_map(|item| item.get("id").and_then(Value::as_str))
        .collect::<Vec<_>>();
    assert!(
        ids.contains(&repo.milestone_id().as_str()),
        "response IDs {ids:?} did not include fixture milestone"
    );

    let status = harness.abort()?;
    if let Some(status) = status {
        assert!(status.success(), "server exited with failure: {status:?}");
    }
    Ok(())
}

#[test]
fn show_flow_matches_expected_id() -> Result<()> {
    let repo = create_repository_fixture()?;
    let milestone_id = repo.milestone_id();
    let mut harness = McpHarness::spawn(repo.path())?;

    harness.initialize()?;
    harness.list_tools()?;

    let response = harness.call_tool(
        "git_mile.show",
        json!({
            "entity": "milestone",
            "id": milestone_id.clone()
        }),
    )?;

    let payload = match response {
        Response::Result(value) => value,
        Response::Error(err) => return Err(anyhow!("git_mile.show returned error: {err:?}")),
    };
    let show_json = extract_content(payload)?;
    assert_eq!(
        show_json.get("id").and_then(Value::as_str),
        Some(milestone_id.as_str())
    );
    let status = harness.abort()?;
    if let Some(status) = status {
        assert!(status.success(), "server exited with failure: {status:?}");
    }
    Ok(())
}

#[test]
fn invalid_filter_returns_json_rpc_error() -> Result<()> {
    let repo = create_repository_fixture()?;
    let mut harness = McpHarness::spawn(repo.path())?;
    harness.initialize()?;
    harness.list_tools()?;

    let response = harness.call_tool(
        "git_mile.list",
        json!({
            "entity": "milestone",
            "filter": "status == ",
            "limit": 5
        }),
    )?;

    match response {
        Response::Result(_) => Err(anyhow!("expected error but got success")),
        Response::Error(err) => {
            assert_eq!(err.get("code").and_then(Value::as_i64), Some(-32602));
            Ok(())
        }
    }
}

#[test]
fn aborted_session_exits_cleanly() -> Result<()> {
    let repo = create_repository_fixture()?;
    let mut harness = McpHarness::spawn(repo.path())?;
    harness.initialize()?;
    harness.list_tools()?;

    let status = harness.abort()?;
    if let Some(status) = status {
        assert!(status.success(), "server exited with failure: {status:?}");
    }
    Ok(())
}
