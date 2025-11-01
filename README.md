# git-mile

git-mile は、`refs/git-mile/tasks/*` 配下の Git コミットとしてタスクイベントを記録するタスクトラッカーです。ワーキングツリーのファイルには触れず、イベントは ULID で識別され、コミットメッセージ本文に JSON で保存されます。

## クレート構成

- `git-mile-core`: タスク ID・イベント・スナップショットのドメインロジック
- `git-mile-store-git`: Git リポジトリへの読み書きを担うストア層
- `git-mile`: CLI エントリポイント

## ビルドとテスト

```bash
cargo fmt
cargo clippy --workspace --all-targets --all-features
cargo test --workspace --all-features
```
