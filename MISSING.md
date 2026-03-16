# MISSING チェックリスト

## 優先度と着手順

1. **High**: README 冒頭の実装範囲説明を現状に合わせる
2. **Medium**: 実装済みだが未記載の CLI/設定項目を README に追記する
3. **Medium**: `docs/02-architecture.md` の計画表現を実装済み表現へ更新する
4. **Low**: 主要導線で未検出だった「doc有/impl無」を継続監視する

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
