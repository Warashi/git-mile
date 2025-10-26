# just lint-pedantic 指摘対応 実行計画（ID-99）

## lint 結果サマリ（取得日: 2025-10-26）

- 実行コマンド: `env CARGO_TERM_COLOR=never just lint-pedantic`
- 主要カテゴリの内訳は以下の通り。件数は同一ルールの警告数。

|分類|主な Clippy ルール|件数|主な発生ファイル例|対応 Issue|
|---|---|---|---|---|
|所有権/Clone 過多|`clippy::redundant_clone`|103|`cli/src/main.rs`, `core/src/{identity,issue,mile}/mod.rs`, `core/tests/property_query.rs`|ID-100|
|API 属性・シグネチャ|`clippy::must_use_candidate` 43件<br>`clippy::missing_const_for_fn` 27件<br>`clippy::needless_pass_by_value` 20件|`core/src/clock/mod.rs`, `core/src/dag/git_backend.rs`, `core/src/query/mod.rs`, `core/src/repo/{cache,sync}.rs`|ID-103|
|format!/Option 簡素化|`clippy::uninlined_format_args` 53件<br>`clippy::redundant_closure` 17件<br>`Option::map_or_else` 系 6件|`cli/src/main.rs`, `core/src/identity/mod.rs`, `core/src/dag/git_backend.rs`, `tests/e2e/src/lib.rs`|ID-102|
|ドキュメント整備|`clippy::missing_errors_doc` 85件<br>`clippy::missing_panics_doc` 5件|`core/src/repo/cache.rs`, `core/src/{identity,issue,mile}/mod.rs`, `core/src/service/{milestones,issues}.rs`, `tests/e2e/src/lib.rs`|ID-101|
|依存バージョン重複・残余 lint|`clippy::multiple_crate_versions` 15件<br>その他: `too_many_lines`, `struct_excessive_bools`, `significant_drop_tightening` など|依存グラフ全体、`core/src/repo/sync.rs`, `core/src/identity/mod.rs` ほか|ID-104|

## カテゴリ別対応方針

### ID-100: redundant clone の解消と所有権整理
- `LamportTimestamp` や `ReplicaId` まわりでの `clone()` 削減を優先し、参照受け渡しへの変更時は呼び出し元のライフタイムを確認。
- CLI・コア API・テスト・ベンチを横断的に見直すため、機能単位でブランチを切り、小さめのコミットに分割。
- 影響範囲が広いため、`cargo test --workspace --all-features` をサブタスク内で段階的に実行しつつ、E2E テストで出力差分を確認。

### ID-101: Result/Panic ドキュメント整備
- 外部公開 API を優先し、実装側のエラー型と panic 条件を洗い出してドキュメント化。
- 既存の `///` コメントに `# Errors` / `# Panics` セクションを追記。重複説明はまとめ、内部モジュールは必要に応じて `#[allow(...)]` も検討。
- `cargo doc -p git_mile_core --no-deps` と `cargo test -p git_mile_core --doc` でレンダリングとサンプルコードを検証。

### ID-102: format!/Option リファクタと表現簡素化
- `format!` 系は `format!("foo {bar}")` 形式へ整理し、重い `format!` 呼び出しは `to_string()` など代替手段も評価。
- `Option`/`Result` チェーンは `map_or_else`, `map_or`, `is_some_and` などで冗長な `if let` を解消。クロージャ渡しは `method` 直接呼び出しへ置換。
- CLI 出力やテストスナップショットの差分が出た場合は期待値更新とレビュー方針の明文化を行う。

### ID-103: API 属性とシグネチャの lint 対応
- `#[must_use]` 追加は呼び出し側の非使用箇所がコンパイル警告になるため、影響範囲を洗い出してフォローアップ。
- `const fn` 化は依存する `const` コンテキスト確認の上で実施し、互換性が崩れる場合は互換レイヤーや feature gate を検討。
- `needless_pass_by_value` は参照化・所有権移動の整理とセットで見直し、呼び出し側に合わせた API 再設計が必要なら変更提案をまとめる。

### ID-104: 依存バージョン重複と残余 lint の整理
- `cargo tree -d` で重複バージョンの発生源を特定し、`[patch]` や依存先のアップデートで解消。サブクレートごとに影響範囲を記録。
- 長大関数 (`too_many_lines`) や `struct_excessive_bools` などはリファクタ方針（関数分割・型導入・allow の是非）を合意した上で実装。
- 依存更新後は `cargo update -p <crate>` の結果を `CHANGELOG` やリリースノートで確認し、破壊的変更がないかチェック。

## 検証フロー

1. 各サブタスクで局所的に `cargo fmt`, `cargo check`, 対象クレートの `cargo test` を実行。
2. lint 対応の塊ごとに `just lint-pedantic` を再実行し、新たな警告の有無を確認。
3. 最終確認として以下を実施し CI と同等の状態を担保する。  
   - `cargo build --workspace --all-features`  
   - `cargo test --workspace --all-features`  
   - `just lint-pedantic`

## 次アクション

- 本計画（ID-99）をコミット後、ID-100 から順番に着手する。  
- 各サブタスクでは影響範囲を小さく保つようコミットを分割し、対応完了時に再度 `just lint-pedantic` でカテゴリ件数を更新してドキュメントへ追記する。

