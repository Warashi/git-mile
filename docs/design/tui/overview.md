# git mile TUI 概要

## 背景

- M4 マイルストーンでは CLI・外部 MCP クライアント・将来の TUI を同一のバックエンド (`git mile mcp-server`) で統合する。
- 既に CLI と MCP サーバーの橋渡しは完了しており、TUI はその上に薄いプレゼンテーション層を築く前提で計画する。
- TUI の実装前に要件・制約・共通化ポイントを明示し、後続タスクで迷わないようにする。

## ゴール

- list/show 操作を TUI から実行できる UX を定義し、`git mile mcp-server` と安全に通信できる設計指針を示す。
- UI レイヤー・MCP クライアントレイヤー・バックエンドサービスの責務を分離し、再利用しやすい API 形に整える。
- 共通テレメトリとエラーハンドリング方針を先に決め、CLI や外部 MCP クライアントと合わせた挙動にする。

## 非ゴール

- TUI の実装そのものをこのドキュメントで完了させること。
- GraphQL や GUI クライアントといった別インターフェイスの詳細設計。
- MCP プロトコル自体の仕様策定（ID-92 のスコープで扱う）。

## 全体アーキテクチャ

```mermaid
flowchart LR
    subgraph CliClients[既存クライアント]
        CLI[git mile CLI]
        Claude[Claude Desktop など]
    end
    subgraph TUIStack[TUI (予定)]
        TuiUI[UI 層<br/>(ratatui)]
        TuiClient[MCP クライアント層]
    end
    CliClients -->|std io| McpServer[(git mile mcp-server)]
    TuiClient -->|std io| McpServer
    TuiUI --> TuiClient
    McpServer --> Core[git_mile_core クエリエンジン]
```

- すべてのクライアントは `git mile mcp-server` に stdio で接続し、MCP メソッド (`git_mile.list` / `git_mile.show`) を呼び出す。
- `git_mile_core` 内の QueryEngine / Service 層は CLI と共通のものを再利用する。
- TUI は単一接続を前提とし、長時間セッションを想定したリトライや再接続フローを別途設計する。

## 関連ドキュメント

- `requirements.md`: 機能・非機能要件および接続要件。
- `architecture.md`: モジュール構成、イベントループ、MCP クライアントの設計方針。
- `risks.md`: リスクと緩和策。
- `backlog.md`: 実装タスク候補と優先度。
