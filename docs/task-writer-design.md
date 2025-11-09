# TaskWriter / TaskStore 設計メモ

## 背景

- CLI の `TaskService::create_with_parents` に代表されるイベント生成ロジックが `crates/git-mile/src/commands/mod.rs:52` で固有実装されており、TUI (`crates/git-mile/src/tui/app.rs:328`) や MCP (`crates/git-mile/src/mcp.rs:492`) も同様の処理を再実装している。
- validation・state kind 解決・親子リンクの二重イベント発行などが各実装で微妙に異なり、バグ温床になっている。
- 3 面で同じロジックをテストする必要があり、テストコストが高い。

## ゴール

1. イベント発行を単一の `TaskWriter` に集約し、CLI/TUI/MCP から再利用できるようにする。
2. `TaskWriter` が依存するストア操作を trait で抽象化し、同期/非同期両面で扱えるようにする。
3. 既存の `TaskPatch` や diff ロジックを共有し、更新パスの整合性を確保する。

## TaskStore trait

```rust
pub trait TaskStore {
    type Error: std::error::Error + Send + Sync + 'static;

    fn append_event(&self, event: &Event) -> Result<Oid, Self::Error>;
    fn load_events(&self, task: TaskId) -> Result<Vec<Event>, Self::Error>;
    fn list_tasks(&self) -> Result<Vec<TaskId>, Self::Error>;
}
```

- 実装例: `GitStore`（同期）。MCP 用には `AsyncTaskStore<S: TaskStore>` ラッパーを提供し、`tokio::task::spawn_blocking` 経由で同期 API を呼び出す。
- CLI/TUI は直接 `TaskStore` を使用。MCP では `TaskWriter` を `Arc<AsyncTaskStore<_>>` と組み合わせる。

## TaskWriter 概要

```rust
pub struct TaskWriter<S> {
    store: S,
    workflow: WorkflowConfig,
}
```

- 主要メソッド:
  - `create_task(&self, CreateTaskRequest) -> Result<CreateTaskResult, TaskWriteError>`
  - `update_task(&self, TaskId, TaskUpdate) -> Result<TaskWriteResult, TaskWriteError>`
  - `set_state(&self, TaskId, Option<String>, Actor) -> Result<TaskWriteResult, TaskWriteError>`
  - `add_comment(&self, TaskId, CommentRequest) -> Result<TaskWriteResult, TaskWriteError>`
  - `link_parents(&self, TaskId, &[TaskId], Actor)` / `unlink_parents`
- `TaskUpdate` は TUI の `TaskPatch` ロジック（`crates/git-mile/src/tui/app.rs:500` 付近）を共有モジュール化したものを想定。

### CreateTaskRequest

```rust
pub struct CreateTaskRequest {
    pub title: String,
    pub state: Option<String>,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    pub description: Option<String>,
    pub parents: Vec<TaskId>,
    pub actor: Actor,
}
```

- `state` が未指定なら `WorkflowConfig::default_state` を適用。
- 親タスクの存在確認は `store.load_events(parent)` で実施し、存在しない場合は `TaskWriteError::MissingParent(parent)` を返す。
- 親子リンクはこれまで通り、子タスク側と親タスク側の両方に `ChildLinked` を書き込む。

### TaskWriteResult

```rust
pub struct TaskWriteResult {
    pub task: TaskId,
    pub events: Vec<Oid>,
}
```

- CLI/TUI ではログ出力やトースト通知に利用。TUI では `TaskWriter` の戻り値を元に `refresh_tasks_with(Some(task))` を呼ぶ。

### エラー設計

```rust
#[derive(thiserror::Error, Debug)]
pub enum TaskWriteError {
    #[error(\"workflow state '{0}' is not allowed\")]
    InvalidState(String),
    #[error(\"parent task {0} not found\")]
    MissingParent(TaskId),
    #[error(\"task {0} not found\")]
    MissingTask(TaskId),
    #[error(\"store error: {0}\")]
    Store(anyhow::Error),
}
```

- `TaskWriter` で `WorkflowConfig::validate_state` を呼び出し、`InvalidState` を一元的に返す。
- ストア層はすべて `Store(anyhow::Error)` にラップして上位へ伝播。

## 統合方針

1. **API/trait 実装**: 新モジュール（例: `crates/git-mile-core/src/app/`）に `TaskWriter`, `TaskUpdate`, `TaskStore` を追加し、単体テストを `git-mile-core` に置く。
2. **CLI 移行**: `TaskService` から `TaskWriter` を compose し、create/comment/state 操作を委譲する（`crates/git-mile/src/commands/mod.rs`）。
3. **TUI 移行**: `App` からイベント生成部分を削除し、`TaskWriter` を保持。既存の `MockStore` を `TaskStore` 実装に変換。
4. **MCP 移行**: 各 tool handler で直接イベントを書いている箇所（`crates/git-mile/src/mcp.rs:492-757` 等）を `TaskWriter`呼び出しに置換。非同期ラッパを適用。

## テスト戦略

- `TaskWriter` のユニットテストで create/update/link/unlink/validation を網羅。
- CLI/TUI/MCP は `TaskWriter` の注入に伴う統合テストを最小限にし、既存テストは動作確認レベルへ縮退。

