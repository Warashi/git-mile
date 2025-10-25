# クエリエンジン

## DSL

クエリは S 式ライクな DSL で表現します。演算子が先頭に来る前置記法で、例として `git mile list issue --filter "(= status \"open\")"` のように指定します。サポートされる比較演算子は `=`, `!=`, `>`, `<`, `>=`, `<=`, `in`, `contains` です。論理演算として `and`, `or`, `not` を利用できます。

## スキーマ

`QuerySchema` はフィールドごとの型情報と許可演算子を保持します。CLI 側はエンティティごとに `issue_schema()` や `milestone_schema()` を注入し、未知のフィールドやサポートされない演算子を早期に弾きます。

## 実行パイプライン

1. DSL を `parse_query` で AST に変換し、バリデーションを通します。
2. `QueryEngine::execute` が AST を評価し、適用可能なフィルタをレコードに対して逐次適用します。
3. ソート指定がある場合は `SortSpec` を複数持ち、`compare_records` が安定ソートを行います。
4. ページネーションは `PageCursor` で実装され、カーソル文字列に offset と cache generation を含めます。カーソルに埋め込まれた世代と現在世代が異なる場合は `QueryError::StaleGeneration` を返し、再実行を促します。

## メトリクス

- `latency.list{entity}`: list コマンド全体の処理時間
- `latency.show{entity}`: show コマンドの処理時間
- `query.ast_parse_time`: DSL 解析に要した時間

これらは `metrics` クレートに集約され、Prometheus 形式で取得できます。

## ベンチマーク

`core/benches/query_latency.rs` に Criterion ベースのベンチマークを追加しました。`cargo bench -p git_mile_core query_latency` で実行し、フィルタとソートのパス性能を確認できます。
