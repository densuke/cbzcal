# cbzcal

サイボウズ Office の予定表を CLI から扱うためのベースプロジェクトです。  
現時点では、公開ドキュメントの整理、日本語の設計資料、TDD 用の `fixture` バックエンド、そして `cybozu-html` による認証、一覧取得、単発予定追加までを用意しています。

## 現状

- `fixture` バックエンドでは、予定の一覧取得・追加・更新・複製・削除をローカル JSON に対して実行できます。
- `cybozu-html` バックエンドでは、Basic 認証 + Cybozu ログイン + `events list` + 単発予定の `events add` / `events update` まで実サイトで確認済みです。
- `cybozu-html` の実サイト向け `events clone` / `events delete` は未実装です。対象サイトの HTML/フォーム契約を採取してから実装を進める前提です。
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

Cybozu 実サイトの一覧取得を試します。

```bash
cargo run -- events list
```

期間を明示したい場合は `--from` / `--to` を付けます。未指定時は JST 当日 00:00 から 1 週間です。

```bash
cargo run -- events list \
  --from 2026-03-09T00:00:00+09:00 \
  --to 2026-03-16T00:00:00+09:00
```

短い書き方も使えます。

```bash
cargo run -- events list --date today
cargo run -- events list --from today --for 7d
```

`fixture` バックエンドで予定を一覧表示します。

```bash
cargo run -- events list \
  --from 2026-03-01T00:00:00+09:00 \
  --to 2026-04-01T00:00:00+09:00
```

`cybozu-html` では現時点で `description` / `attendees` / `facility` は一覧から未抽出です。  
また、グループ週表示では共有予定が参加者ごとに重複して見えるため、CLI では `sEID + Date + BDate` 単位で 1 件に畳み、さらに現在ユーザーの `UID` 行だけを取得対象にしています。

予定を追加します。

```bash
cargo run -- events add \
  --title "設計レビュー" \
  --start 2026-03-10T10:00:00+09:00 \
  --end 2026-03-10T11:00:00+09:00 \
  --description "CLI 基盤の確認"
```

短い書き方では、次のように日付と時刻を分けて指定できます。

```bash
cargo run -- events add \
  --title "設計レビュー" \
  --date 3/10 \
  --at 9 \
  --until 11 \
  --description "CLI 基盤の確認"

cargo run -- events add \
  --title "設計レビュー" \
  --date 3/11 \
  --at 9 \
  --for 2h

cargo run -- events add \
  --title "終日予定" \
  --date today
```

`cybozu-html` の `events add` は現時点で通常予定の単日登録のみ対応です。`--attendee`、`--facility`、`--calendar`、日付またぎ予定はまだ扱えません。

予定を更新します。

```bash
cargo run -- events update \
  --id 'sEID=3096804&UID=379&GID=183&Date=da.2099.1.7&BDate=da.2099.1.5' \
  --title "更新後のタイトル" \
  --start 2099-01-07T13:00:00+09:00 \
  --end 2099-01-07T14:30:00+09:00 \
  --description "更新後メモ"
```

`cybozu-html` の `events update` は現時点で通常予定の単日更新のみ対応です。更新できるのは `title` / `description` / `start` / `end` だけで、`--attendee`、`--facility`、`--calendar`、繰り返し予定はまだ扱えません。

明示的に別ファイルを使いたい場合は `--config /path/to/config.toml` を指定します。  
対応形式は `.yml`、`.yaml`、`.toml` です。

一時的に認証情報を直書きしたい場合は、`.cbzcal.toml` または `.cbzcal.yml` に `basic_username` / `basic_password` と `office_username` / `office_password` を置けます。  
ただし平文資格情報は `0400` / `0600` 前提で、環境変数設定がある場合は環境変数が優先されます。

## テスト

```bash
cargo test
```
