# テスト戦略

## 単体・統合テスト

既存のユニットテストに加えて、CLI の主要フロー (作成・一覧・詳細表示) を統合テストでカバーしています。キャッシュとクエリエンジンの振る舞いは `git_mile_core` のユニットテストで検証します。

## プロパティテスト

`cargo test -p git_mile_core --features property-tests` でプロパティテストを実行できます。

- `property_cache.rs`: 永続キャッシュが単純な参照実装と同じ挙動になることを検証
- `property_query.rs`: DSL で構築したフィルタが期待通りの `IssueDetails` を返すことを確認
- パーサーが任意文字列でも panic しないことをチェック

## ファズ (準備)

`property_query.rs` のランダム入力テストは軽量な smoke fuzz として機能します。将来的に `cargo fuzz` を導入する場合は `proptest` の戦略を corpus として流用してください。

## ドキュメント整合性

`docs/` 配下にキャッシュ構成とクエリエンジン、テスト戦略のまとめを追加しました。チーム内レビュー時にはこれらの Markdown を参照し、CI では markdownlint などの静的検査を追加予定です。
