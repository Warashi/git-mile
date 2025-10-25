# キャッシュアーキテクチャ

## 全体像

`git mile` の CLI は RocksDB をバックエンドにした永続キャッシュを利用します。エンティティ種別ごとに Column Family を分割し、キーは `EntityId`、値は CBOR でシリアライズした `EntitySnapshot` です。キャッシュ・ヒット率や再構築時間は `metrics` クレート経由で収集し、Prometheus 形式でエクスポートできます。

## 世代管理

各 Namespace は `IndexGeneration` を持ち、`on_pack_persisted` が呼ばれるたびに世代をローテーションします。世代 ID は RocksDB のメタデータに保存され、古い世代のスナップショットは `CacheLoadOutcome::Stale` として扱われます。CLI のページネーションカーソルには世代 ID が埋め込まれ、背景同期による整合性崩れを防ぎます。

## ジャーナルと同期

`PersistentCache` は `cache.journal` CF にデルタを書き込み、`BackgroundSyncWorker` が非同期で処理します。ワーカーのキュー長は `sync.queue_depth` としてゲージに公開され、適切に drain されているか監視できます。ワーカーは適用結果を `pending`/`applied`/`failed` で管理し、失敗したデルタは再実行対象として残ります。

## エラーハンドリング

キャッシュの読み込み時には CRC32 による整合性チェックと TTL 判定を行い、破損エントリは自動で削除します。書き込みエラーは指数バックオフ付きでリトライし、最終的にフォールバックとして再取得を行います。

## メトリクス

- `cache.requests{namespace, outcome}`: ヒット、ミス、ステールの内訳
- `cache.rebuild_latency{namespace}`: スナップショット再構築の所要時間
- `cache.evictions{namespace}`: 明示的な無効化回数
- `sync.queue_depth{namespace}`: バックグラウンドキューの長さ

CLI 起動時に `git_mile_core::metrics::init_prometheus()` を呼び出すことでレコーダーを初期化できます。`git mile metrics dump` でスナップショットをその場で出力可能です。
