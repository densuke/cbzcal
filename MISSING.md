# MISSING チェックリスト

## 優先度と着手順

1. **High**: README 冒頭の実装範囲説明を現状に合わせる
2. **Medium**: 実装済みだが未記載の CLI/設定項目を README に追記する
3. **Medium**: `docs/02-architecture.md` の計画表現を実装済み表現へ更新する
4. **Low**: 主要導線で未検出だった「doc有/impl無」を継続監視する

## 実施計画（チェックリスト）

### Phase 0: 事前準備（直列）

- [ ] `main` を最新化し、作業前にワークツリー方針を確認する
- [ ] 競合回避のため、README を触る作業範囲を A/B で明確に分離する
  - A: README 冒頭の実装範囲説明（High）
  - B: README の CLI/設定項目追記（Medium）

### Phase 1: High 対応（単独または先行実施）

- [ ] README 冒頭の実装範囲説明を現状実装に合わせる
- [ ] 差分レビューを行い、コミットする（High 専用コミット）

### Phase 2: Medium 対応（並列可能）

- [ ] 【並列可: Track B】README に `shell` / `--no-cache` / `events_cache_path` を追記する
- [ ] 【並列可: Track C】`docs/02-architecture.md` の「実装されたら」表現を実装済み表現へ更新する
- [ ] 各 Track ごとに差分レビューし、独立コミットを作成する

### Phase 3: 統合確認（直列）

- [ ] 全コミット適用後に `MISSING.md` の High/Medium 項目を完了へ更新する
- [ ] README と `docs/02-architecture.md` の記述整合性を最終確認する
- [ ] 「doc 有 / impl 無」の再照合を実施し、Low 項目の状態を更新する

### 並列実行の目安

- [ ] 並列可能: Phase 2 の Track B（README 追記）と Track C（architecture 更新）
- [ ] 非並列推奨: README 冒頭（High）と README 追記（Medium）を同時編集する運用
  - 理由: 同一ファイル競合とレビュー負荷を避けるため

## Worktree 実行テンプレート（実施時に使用）

### ブランチ / ワークツリー割り当て

- [ ] WT-A（High）: README 冒頭の実装範囲更新
  - branch 例: `docs/missing-high-readme-scope`
  - 対象: `README.md` 冒頭の現状説明のみ

- [ ] WT-B（Medium-README）: README への未記載項目追記
  - branch 例: `docs/missing-medium-readme-cli-config`
  - 対象: `shell`, `--no-cache`, `events_cache_path`

- [ ] WT-C（Medium-Architecture）: architecture 文言更新
  - branch 例: `docs/missing-medium-architecture-wording`
  - 対象: `docs/02-architecture.md` の計画表現

### 実行順（推奨）

- [ ] 1. WT-A を先に完了（README 冒頭のみ編集）
- [ ] 2. WT-B / WT-C を並列で実施
- [ ] 3. 各 branch を個別レビューして順次マージ
- [ ] 4. 最後に `MISSING.md` の該当チェックを更新

### レビュー観点（各 WT 共通）

- [ ] 記述が実装仕様（`src/cli.rs`, `src/config.rs`, `src/backend/cybozu_html.rs`）と一致している
- [ ] 用語・表現が README / docs 間で矛盾していない
- [ ] 変更範囲が担当テーマから逸脱していない（不要編集なし）

## High

- [x] README 冒頭の実装範囲説明が古い
  - 記載: `README.md` 冒頭で「`cybozu-html` による認証、一覧取得、単発予定追加まで」
  - 実装/同README内現状: `events clone` / `events update` / `events delete`（繰り返しの `update/delete` 含む）まで対応
  - 影響: 初見ユーザーが「どこまで使えるか」を誤認しやすい

## Medium

- [ ] `shell` サブコマンドがドキュメント未記載
  - 実装: `src/cli.rs` の `Command::Shell`、`src/app.rs` の `Command::Shell` 分岐
  - 現状ドキュメント: `README.md` / `docs/02-architecture.md` のコマンド一覧に未記載

- [ ] グローバルオプション `--no-cache` がドキュメント未記載
  - 実装: `src/cli.rs` の `Cli.no_cache`、`src/app.rs` -> `build_backend(..., cli.no_cache, ...)`
  - 現状ドキュメント: `README.md` に利用方法・意味・注意点の記載なし

- [ ] 設定項目 `events_cache_path` がドキュメント未記載
  - 実装: `src/config.rs` の `AppConfig.events_cache_path` と `events_cache_path()`
  - 現状ドキュメント: `cbzcal.example.toml` と `README.md` に設定キー説明なし

- [ ] `docs/02-architecture.md` の `cybozu-html` 記述が計画時の表現のまま
  - 記載: 「`cybozu-html` が実装されたら、最低でも次を担当します。」
  - 実装: `src/backend/cybozu_html.rs` で実装済み（一覧/追加/更新/複製/削除の主要経路あり）

## Low

- [ ] 「ドキュメントにあるのに実装されていない」機能仕様は、主要ユーザー導線（`doctor` / `probe-login` / `events list|add|update|clone|delete`）では未検出
  - 補足: 現時点では未検出だが、機能追加のたびに再照合を実施する
