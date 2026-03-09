# リファクタリング計画 (UNIX哲学 & 単一責任の原則)

## タスクリスト

- [x] 1. 日時・期間ロジックの集約 (`src/datetime.rs`)
    - [x] `cli.rs`, `app.rs`, `prompt.rs`, `backend/mod.rs` からロジックを抽出
    - [x] 単体テストの移行と整備
- [x] 2. 表示・レンダリングロジックの分離 (`src/view.rs`)
    - [x] `app.rs` からテキスト/JSONフォーマット処理を抽出
- [x] 3. ブラウザ操作の抽象化 (`src/browser.rs`)
    - [x] OS依存の起動ロジックをカプセル化
- [x] 4. 診断（Doctor）ロジックの分離 (`src/doctor.rs`)
    - [x] `config.rs` から診断処理を抽出
- [x] 5. 実行ロジックのモジュール化 (`src/executor.rs`)
    - [x] `app.rs` の巨大な match 式を分割
- [x] 6. ID解析ロジックの整理 (`src/backend/id.rs`)
    - [x] ドメインモデルからのバックエンド依存の除去
- [x] 7. 全体のクリーンアップと依存関係の整理
    - [x] `lib.rs` でのモジュール公開設定の適正化
    - [x] 結合テストによる最終確認
