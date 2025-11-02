# git-mile

git-mile は、`refs/git-mile/tasks/*` 配下の Git コミットとしてタスクイベントを記録するタスクトラッカーです。ワーキングツリーのファイルには触れず、イベントは ULID で識別され、コミットメッセージ本文に JSON で保存されます。

## クレート構成

- `git-mile-core`: タスク ID・イベント・スナップショットのドメインロジック
- `git-mile-store-git`: Git リポジトリへの読み書きを担うストア層
- `git-mile`: CLI エントリポイント

## データモデル

- ラベル・担当者・リレーションは [`crdts`](https://docs.rs/crdts/) の ORSWOT (Observed-Remove Set Without Tombstones) で表現し、オフライン同時編集でも自然に結合できます。
- タイトル・状態・説明といった単一値は同クレートの LWW レジスタで保持し、イベントのタイムスタンプと ULID によるトータルオーダーで収束します。
- スナップショットは CRDT の結果を投影したビューであり、`TaskSnapshot::replay`・`TaskSnapshot::apply` のどちらでも一貫した状態が取得できます。

## ビルドとテスト

```bash
cargo fmt
cargo clippy --workspace --all-targets --all-features
cargo test --workspace --all-features
```
