use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use assert_cmd::cargo::CommandCargoExt;
use git_mile_core::clock::ReplicaId;
use git_mile_core::issue::{CreateIssueInput, IssueId, IssueStatus, IssueStore};
use git_mile_core::mile::{CreateMileInput, MileId, MileStatus, MileStore};
use git_mile_core::repo::LockMode;
use git2::Repository;
use serde_json::{Map, Value, json};
use tempfile::TempDir;

pub struct TestRepository {
    temp: TempDir,
    milestone_id: MileId,
    issue_id: IssueId,
}

impl TestRepository {
    #[must_use]
    pub fn path(&self) -> &Path {
        self.temp.path()
    }

    #[must_use]
    pub fn milestone_id(&self) -> String {
        self.milestone_id.to_string()
    }

    #[must_use]
    pub fn issue_id(&self) -> String {
        self.issue_id.to_string()
    }
}

/// Create a repository fixture populated with an issue and a milestone.
///
/// # Errors
///
/// Returns an error when the repository cannot be created or seeded with the initial data.
pub fn create_repository_fixture() -> Result<TestRepository> {
    let temp = tempfile::tempdir().context("failed to create temp dir")?;
    Repository::init_bare(temp.path()).context("failed to init bare repo")?;
    let replica = ReplicaId::new("mcp-e2e");
    let milestone_id = create_milestone(temp.path(), &replica).context("create milestone")?;
    let issue_id = create_issue(temp.path(), &replica).context("create issue")?;
    Ok(TestRepository {
        temp,
        milestone_id,
        issue_id,
    })
}

fn create_milestone(repo: &Path, replica: &ReplicaId) -> git_mile_core::error::Result<MileId> {
    let store = MileStore::open_with_mode(repo, LockMode::Write)?;
    let snapshot = store.create_mile(CreateMileInput {
        replica_id: replica.clone(),
        author: "Tester <tester@example.com>".into(),
        message: Some("create milestone".into()),
        title: "Milestone Alpha".into(),
        description: Some("First milestone".into()),
        initial_status: MileStatus::Open,
        initial_comment: Some("kickoff".into()),
        labels: vec!["alpha".into()],
    })?;
    Ok(snapshot.id)
}

fn create_issue(repo: &Path, replica: &ReplicaId) -> git_mile_core::error::Result<IssueId> {
    let store = IssueStore::open_with_mode(repo, LockMode::Write)?;
    let snapshot = store.create_issue(CreateIssueInput {
        replica_id: replica.clone(),
        author: "Tester <tester@example.com>".into(),
        message: Some("create issue".into()),
        title: "Issue Alpha".into(),
        description: Some("Issue details".into()),
        initial_status: IssueStatus::Open,
        initial_comment: Some("Initial comment".into()),
        labels: vec!["alpha".into()],
    })?;
    Ok(snapshot.id)
}

pub struct McpHarness {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    stderr_handle: Option<thread::JoinHandle<String>>,
    buffer: String,
    next_id: i64,
}

pub enum Response {
    Result(Value),
    Error(Value),
}

impl McpHarness {
    /// Spawn a `git-mile` MCP server process and connect to it.
    ///
    /// # Errors
    ///
    /// Returns an error when the process cannot be started or any of the stdio handles cannot be
    /// captured.
    pub fn spawn(repo_path: &Path) -> Result<Self> {
        let mut cmd = Command::cargo_bin("git-mile")?;
        cmd.arg("mcp-server")
            .arg("--repo")
            .arg(repo_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().context("failed to spawn git mile mcp-server")?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("failed to capture stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("failed to capture stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("failed to capture stderr"))?;

        let stderr_handle = thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut logs = String::new();
            let _ = reader.read_to_string(&mut logs);
            logs
        });

        Ok(Self {
            child: Some(child),
            stdin: Some(stdin),
            stdout: BufReader::new(stdout),
            stderr_handle: Some(stderr_handle),
            buffer: String::new(),
            next_id: 1,
        })
    }

    /// Send the MCP `initialize` request and wait for the response.
    ///
    /// # Errors
    ///
    /// Returns an error when the request cannot be sent or the server rejects the initialization.
    pub fn initialize(&mut self) -> Result<Response> {
        let params = json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {}
            },
            "clientInfo": {
                "name": "git-mile-e2e",
                "version": "0.1.0"
            }
        });
        match self.request("initialize", Some(params))? {
            Response::Result(info) => {
                self.send_notification(
                    "notifications/initialized",
                    Some(Value::Object(Map::new())),
                )?;
                Ok(Response::Result(info))
            }
            Response::Error(error) => Ok(Response::Error(error)),
        }
    }

    /// Request the list of tools exposed by the MCP server.
    ///
    /// # Errors
    ///
    /// Returns an error when the request cannot be sent or the response cannot be parsed.
    pub fn list_tools(&mut self) -> Result<Response> {
        let params = Value::Object(Map::new());
        self.request("tools/list", Some(params))
    }

    /// Invoke a specific MCP tool with the provided arguments.
    ///
    /// # Errors
    ///
    /// Returns an error when the request cannot be sent or the response cannot be parsed.
    pub fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Response> {
        let mut params = Map::new();
        params.insert("name".into(), Value::String(name.to_string()));
        params.insert("arguments".into(), arguments);
        self.request("tools/call", Some(Value::Object(params)))
    }

    /// Shut down the MCP server gracefully and return its exit status.
    ///
    /// # Errors
    ///
    /// Returns an error when the shutdown request fails or the process does not exit cleanly.
    pub fn shutdown(mut self) -> Result<ExitStatus> {
        let response = self.request("shutdown", Some(Value::Null))?;
        match response {
            Response::Result(value) => {
                if !value.is_null() {
                    return Err(anyhow!("shutdown result must be null: {value:?}"));
                }
            }
            Response::Error(err) => {
                return Err(anyhow!("shutdown returned error: {err:?}"));
            }
        }
        self.close_stdin();
        let status = self.wait_for_exit(Duration::from_secs(5))?;
        let logs = stderr_logs_to_option(self.stderr_handle.take());
        self.finish(logs);
        Ok(status)
    }

    /// Terminate the MCP server without sending a shutdown request.
    ///
    /// # Errors
    ///
    /// Returns an error when the process status or stderr logs cannot be collected.
    pub fn abort(mut self) -> Result<Option<ExitStatus>> {
        self.close_stdin();
        let status = self.wait_for_exit(Duration::from_secs(5)).ok();
        let logs = stderr_logs_to_option(self.stderr_handle.take());
        self.finish(logs);
        Ok(status)
    }

    fn finish(&mut self, logs: Option<String>) {
        if let Some(mut child) = self.child.take() {
            let _ = child.wait();
        }
        if let Some(logs) = logs.filter(|s| !s.trim().is_empty()) {
            eprintln!("mcp-server stderr:\n{logs}");
        }
    }

    fn request(&mut self, method: &str, params: Option<Value>) -> Result<Response> {
        let id = self.next_id;
        self.next_id += 1;
        let mut message = Map::new();
        message.insert("jsonrpc".into(), Value::String("2.0".into()));
        message.insert("id".into(), Value::Number(id.into()));
        message.insert("method".into(), Value::String(method.into()));
        if let Some(params) = params {
            message.insert("params".into(), params);
        }
        self.send(&Value::Object(message))?;
        self.recv_response(id)
    }

    fn send(&mut self, payload: &Value) -> Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow!("stdin already closed"))?;
        let serialized = serde_json::to_string(payload)?;
        if let Err(err) = serde_json::from_str::<rmcp::model::JsonRpcMessage>(&serialized) {
            return Err(anyhow!("invalid MCP message {serialized}: {err}"));
        }
        stdin.write_all(serialized.as_bytes())?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;
        Ok(())
    }

    fn recv_response(&mut self, expected_id: i64) -> Result<Response> {
        loop {
            let value = self.recv_message()?;
            match (
                value.get("id").and_then(Value::as_i64),
                value.get("result"),
                value.get("error"),
            ) {
                (Some(id), Some(result), _) if id == expected_id => {
                    return Ok(Response::Result(result.clone()));
                }
                (Some(id), _, Some(error)) if id == expected_id => {
                    return Ok(Response::Error(error.clone()));
                }
                _ => {}
            }
        }
    }

    fn send_notification(&mut self, method: &str, params: Option<Value>) -> Result<()> {
        let mut message = Map::new();
        message.insert("jsonrpc".into(), Value::String("2.0".into()));
        message.insert("method".into(), Value::String(method.into()));
        if let Some(params) = params {
            message.insert("params".into(), params);
        }
        self.send(&Value::Object(message))
    }

    fn recv_message(&mut self) -> Result<Value> {
        loop {
            self.buffer.clear();
            let read = self
                .stdout
                .read_line(&mut self.buffer)
                .context("failed to read server output")?;
            if read == 0 {
                return Err(anyhow!("server closed stdout"));
            }
            let trimmed = self.buffer.trim();
            if trimmed.is_empty() {
                continue;
            }
            let value = serde_json::from_str(trimmed)
                .with_context(|| format!("invalid json from server: {trimmed}"))?;
            return Ok(value);
        }
    }

    fn close_stdin(&mut self) {
        if let Some(mut stdin) = self.stdin.take() {
            let _ = stdin.flush();
            drop(stdin);
        }
    }

    fn wait_for_exit(&mut self, timeout: Duration) -> Result<ExitStatus> {
        let child = self
            .child
            .as_mut()
            .ok_or_else(|| anyhow!("child already collected"))?;
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = child.try_wait()? {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                child.kill().ok();
                let status = child.wait()?;
                return Ok(status);
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for McpHarness {
    fn drop(&mut self) {
        self.close_stdin();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(logs) = self
            .stderr_handle
            .take()
            .and_then(|handle| handle.join().ok())
            .filter(|logs| !logs.trim().is_empty())
        {
            eprintln!("mcp-server stderr:\n{logs}");
        }
    }
}

fn stderr_logs_to_option(handle: Option<thread::JoinHandle<String>>) -> Option<String> {
    handle.and_then(|h| h.join().ok())
}
