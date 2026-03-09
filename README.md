# cbzcal

サイボウズ Office の予定表を CLI から扱うためのベースプロジェクトです。  
現時点では、公開ドキュメントの整理、日本語の設計資料、TDD 用の `fixture` バックエンド、そして将来の `cybozu-html` 実装の足場までを用意しています。

## 現状

- `fixture` バックエンドでは、予定の一覧取得・追加・更新・複製・削除をローカル JSON に対して実行できます。
- `cybozu-html` バックエンドでは、Basic 認証とサイボウズ Office の画面遷移を扱うための設定枠を用意しています。
- 実サイト向け CRUD は未実装です。対象サイトの HTML/フォーム契約を採取してから実装を進める前提です。
- `cybozu-html` の設定は、接続先 `base_url`、前段 Basic 認証、Cybozu ログイン画面 URL、Cybozu 本体ログイン資格情報を分けて持つ想定です。
- 設定ファイルは `.cbzcal.toml` を標準とし、探索順は `カレントディレクトリ -> XDG_CONFIG_HOME/cbzcal/config.toml -> ~/.cbzcal.toml` です。
- YAML も読めますが、各場所で `.toml` を先に見て、なければ `.yml` を見ます。
- Unix 系では、設定ファイルの権限が `0400` または `0600` でないと起動しません。
- 認証情報は環境変数参照が標準ですが、一時検証用に設定ファイルへ直接書く fallback も使えます。

## ドキュメント

- [調査メモ](docs/01-research.md)
- [アーキテクチャ](docs/02-architecture.md)
- [開発フロー](docs/03-development-flow.md)
- [ブラウザ調査計画](docs/04-browser-investigation.md)

`docs/01-research.md` には、2026-03-09 時点で採取できた認証フローと予定画面の実観測結果を追記しています。

## 使い方

設定ファイルの例をコピーします。

```bash
cp cbzcal.example.toml .cbzcal.toml
chmod 600 .cbzcal.toml
```

設定内容を確認します。

```bash
cargo run -- doctor
```

Cybozu 実サイトへのヘッドレスログインと `ScheduleIndex` 到達を確認します。

```bash
cargo run -- probe-login
```

`fixture` バックエンドで予定を一覧表示します。

```bash
cargo run -- events list \
  --from 2026-03-01T00:00:00+09:00 \
  --to 2026-04-01T00:00:00+09:00
```

予定を追加します。

```bash
cargo run -- events add \
  --title "設計レビュー" \
  --start 2026-03-10T10:00:00+09:00 \
  --end 2026-03-10T11:00:00+09:00 \
  --description "CLI 基盤の確認" \
  --attendee alice \
  --attendee bob \
  --facility 会議室A \
  --calendar 開発
```

明示的に別ファイルを使いたい場合は `--config /path/to/config.toml` を指定します。  
対応形式は `.yml`、`.yaml`、`.toml` です。

一時的に認証情報を直書きしたい場合は、`.cbzcal.toml` または `.cbzcal.yml` に `basic_username` / `basic_password` と `office_username` / `office_password` を置けます。  
ただし平文資格情報は `0400` / `0600` 前提で、環境変数設定がある場合は環境変数が優先されます。

## テスト

```bash
cargo test
```
