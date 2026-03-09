# cbzcal

サイボウズ Office の予定表を CLI から扱うためのベースプロジェクトです。  
現時点では、公開ドキュメントの整理、日本語の設計資料、TDD 用の `fixture` バックエンド、そして `cybozu-html` による認証、一覧取得、単発予定追加までを用意しています。

## 現状

- `fixture` バックエンドでは、予定の一覧取得・追加・更新・複製・削除をローカル JSON に対して実行できます。
- `cybozu-html` バックエンドでは、Basic 認証 + Cybozu ログイン + `events list`、通常予定の `events add` / `events clone`、通常予定と繰り返し予定の `events update` / `events delete` まで実サイトで確認済みです。
- `cybozu-html` の設定は、接続先 `base_url`、前段 Basic 認証、Cybozu ログイン画面 URL、Cybozu 本体ログイン資格情報を分けて持つ想定です。
- `events --prompt "..."` で自然文から `list` / `add` / `update` / `clone` / `delete` の引数生成を試せます。既定では必ず確認し、`--yes` は `list` / `add` / `clone` のみ省略可能です。
- `cybozu-html` はログイン後の Cookie をローカルに保存し、次回起動時に再利用します。既定保存先は `XDG_STATE_HOME/cbzcal/session-cookies.json`、未設定時は `~/.local/state/cbzcal/session-cookies.json` です。
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

`cybozu-html` は 1 回目のログイン成功後に Cookie を保存し、次回以降は有効なセッションが残っていれば再ログインを省略します。保存先を変えたい場合は `session_cache_path` を設定します。

Cybozu 実サイトの一覧取得を試します。既定では人間向けのテキスト表示です。

```bash
cargo run -- events list
cargo run -- events
```

JSON が必要な場合は `--json` を付けます。

```bash
cargo run -- events list --json
```

認証経路やセッション再利用の補助情報を見たい場合は `-v` を付けます。`-v` の出力は標準エラーに出ます。

```bash
cargo run -- -v events list --date today
```

自然文から引数を組み立てたい場合は `--prompt` を使います。実行前に解釈結果と生成コマンドを表示し、既定では `[y/N]` で確認します。
`非公開で` のような文言は `events add --private` として解釈します。明示がなければ公開です。

```bash
cargo run -- events --prompt "明日の15時から1時間、『伊藤様と打ち合わせ』で追加"
cargo run -- events --prompt "明日の17時半から3時間、非公開で『ミーティング』を設定"
```

`--yes` を付けると確認を省略できますが、これは `list` / `add` / `clone` の prompt 実行だけで有効です。`update` / `delete` では常に確認が必要です。

```bash
cargo run -- events --prompt "明日の予定を表示" --yes
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

Cybozu 系イベントの短縮 ID は `sEID@YYYY-MM-DD` 形式で、`update` / `clone` / `delete` にそのまま渡せます。テキスト表示でも各行の末尾に `[...]` で表示します。JSON 出力では `short_id` フィールドを含めます。

予定を追加します。

`events add` / `update` / `clone` / `delete` も、既定では人間向けの結果表示です。JSON が必要な場合だけ `--json` を付けます。

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
  --private \
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
`--public` / `--private` で予定の公開方法を切り替えられます。既定は `--public` 相当です。

予定を更新します。

```bash
cargo run -- events update \
  --id '3096804@2099-01-07' \
  --title "更新後のタイトル" \
  --start 2099-01-07T13:00:00+09:00 \
  --end 2099-01-07T14:30:00+09:00 \
  --description "更新後メモ"

cargo run -- events update \
  --id '2570212@2026-03-09' \
  --scope this \
  --title "今回だけ変更"

cargo run -- events update \
  --id '3092194@2026-03-09' \
  --web
```

`cybozu-html` の `events update` は `title` / `description` / `start` / `end` のみ更新できます。繰り返し予定では `--scope this|after|all` を使えます。`--web` を付けると該当予定の画面をブラウザで開き、patch 指定がある場合は headless 更新後に開きます。`--attendee`、`--facility`、`--calendar` はまだ扱えません。

予定を複製します。

```bash
cargo run -- events clone \
  --id '3096828@2099-01-10' \
  --title-suffix " (複製)" \
  --start 2099-01-11T14:00:00+09:00
```

`cybozu-html` の `events clone` は現時点で通常予定の単日複製のみ対応です。`short_id` から元予定を解決し、`ScheduleEntry?mode=reuse` を使って複製します。参加者・施設・繰り返し予定はまだ扱えません。公開方法は元予定を引き継ぎます。

予定を削除します。

```bash
cargo run -- events delete \
  --id '3096804@2099-01-07'

cargo run -- events delete \
  --id '2570212@2026-03-09' \
  --scope this
```

`cybozu-html` の `events delete` は通常予定に加えて、繰り返し予定では `--scope this|after|all` を使えます。複数参加者予定の `self` 離脱はまだ扱えません。

明示的に別ファイルを使いたい場合は `--config /path/to/config.toml` を指定します。  
対応形式は `.yml`、`.yaml`、`.toml` です。

一時的に認証情報を直書きしたい場合は、`.cbzcal.toml` または `.cbzcal.yml` に `basic_username` / `basic_password` と `office_username` / `office_password` を置けます。  
ただし平文資格情報は `0400` / `0600` 前提で、環境変数設定がある場合は環境変数が優先されます。

`--prompt` でローカル LLM を使う場合は、必要に応じて設定ファイルに `ollama` セクションを追加します。未指定時は `http://127.0.0.1:11434` と `gemma3:4b` を使います。

```toml
[ollama]
base_url = "http://127.0.0.1:11434"
model = "gemma3:4b"
```

## テスト

```bash
cargo test
```
