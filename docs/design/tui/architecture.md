# TUI アーキテクチャ

## コンポーネント構成

| レイヤー | 主な責務 | 概要 |
| --- | --- | --- |
| `ui` | 入力処理・描画 | `ratatui` を利用し、キーバインドとレンダリングを管理する。 |
| `session` | MCP クライアント | `rmcp` を Async I/O でラップし、`initialize` / `list` / `show` を非同期に呼び出す。 |
| `store` | キャッシュ | list 結果や show 詳細を保持し、UI からの更新要求に応じて差分描画する。 |
| `app` | オーケストレーション | 状態管理、コマンド実行キュー、エラーハンドリングを統括する。 |

## イベントループ

```text
┌──────────┐       ┌──────────┐       ┌──────────┐
│ Terminal │──────▶│ UI Layer │──────▶│ Command  │
│  Input   │       │ (ratatui)│       │  Queue   │
└──────────┘◀──────┴──────────┘◀──────┴──────────┘
                               ▲
                               │
                         ┌──────────┐
                         │ MCP      │
                         │ Session  │
                         └──────────┘
```

- UI レイヤーは `crossterm` のイベント (`KeyEvent`, `Resize`) を受け取ってアクションへ変換する。
- アクションは `CommandQueue` に登録され、Tokio Runtime 上で順次処理される。
- MCP セッションはバックグラウンドタスクとして起動し、レスポンスをチャネル経由で UI に返す。

## MCP クライアント

- `rmcp::client::stdio::Client` を子プロセスとの通信に利用する。
- `initialize` 時に capability を記録し、未サポートメソッドは UI が disable 状態にする。
- `list` のページングは `next_cursor` を保持して「もっと見る」を提供する設計とする。
- `show` は詳細情報を取得次第キャッシュし、再訪時はキャッシュから即時表示する。

## エラーハンドリング

- MCP 側の `ErrorData` は `category` を分類 (`InvalidInput`, `NotFound`, `Internal`) し、UI に適したメッセージへ変換する。
- セッション切断時は自動再試行を 1 回まで行い、失敗したら UI にリトライダイアログを表示する。
- 連続失敗が 3 回続いた場合は `Backoff` 状態に入り、ユーザー操作があるまで新規リクエストを送らない。

## 依存ライブラリ想定

- `ratatui` + `crossterm`: 描画と入力処理。
- `tokio` + `tokio-stream`: 非同期イベントの多重化。
- `serde` / `serde_json`: MCP メッセージのデコード。
- `color-eyre`: UI 層での詳細なエラーメッセージ表示を補助。

## モジュール構造案

```
git-mile-tui/
├── src/
│   ├── app.rs
│   ├── ui/
│   │   ├── layout.rs
│   │   └── components/
│   ├── session/
│   │   ├── mod.rs
│   │   └── client.rs
│   ├── store.rs
│   └── telemetry.rs
└── resources/
    └── themes/
```

- MVP では `git-mile-tui` を独立クレートとして実装し、CLI とは別バイナリ（`git-mile-tui`）を生成する想定。
- 将来的に CLI (`git mile tui`) と統合する場合でも、モジュラリティを保てる構成にする。
