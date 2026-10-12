# QRSend CLI コマンドリファレンス

`qrsend` の全コマンドと全オプションの解説。使い方の流れは [README](../README.md)、仕組みは [PROTOCOL.md](PROTOCOL.md) を参照。

どのコマンドも `--help` で同じ内容（英語）を表示する（`qrsend send --help` など。`-h` は短い版）。この文書は、コマンドやオプションが増えたらテスト（`crates/qrsend/tests/reference.rs`）が落ちるので、実物とずれない。

## コマンド一覧

| コマンド | すること |
|---|---|
| [`qrsend send`](#qrsend-send) | ファイル・フォルダ・テキストを QR コードのストリームとして表示する（または動画・画像・テキストに書き出す） |
| [`qrsend recv`](#qrsend-recv) | カメラ・動画・画像・テキストからストリームを読んで受け取る |
| [`qrsend id`](#qrsend-id) | このデバイスの ID を表示する（相手がこのデバイス宛てに暗号化するためのもの） |
| [`qrsend devices`](#qrsend-devices) | 信頼済みデバイスの管理（`list` / `add` / `rm` / `rename`） |
| [`qrsend inbox`](#qrsend-inbox) | 受け取ったもの（途中のものを含む）の管理（`list` / `show` / `export` / `missing` / `rm`） |
| [`qrsend cache`](#qrsend-cache) | 送り直し用に残している送信データの管理（`list` / `clean`） |
| [`qrsend storage`](#qrsend-storage) | 残しているもの全部の容量の一覧と削除（`list` / `clean`） |
| [`qrsend log`](#qrsend-log) | 直近の `send` / `recv` の診断ログを表示する（`show` / `list` / `clear`） |
| [`qrsend completions`](#qrsend-completions) | シェルの補完スクリプトを出力する |

共通のオプションは `-h` / `--help`（ヘルプ）と、`qrsend` 自体の `-V` / `--version`（版）だけ。

### よくある流れ

```bash
# 1. 最初に 1 回だけ: 受け取る側で ID を表示し、送る側で登録する
qrsend id                                   # 受け取る側
qrsend devices add 'qrsend-id:1:age1…'      # 送る側

# 2. 送る（ウィンドウに QR コードが流れる）
qrsend send photos/ notes.md --to phone

# 3. 受け取る
qrsend recv --camera -o ~/Downloads

# 4. 途中で止まったら、受け取る側に出た resume code を送る側に入れる
qrsend send --resume QSR1-XXXXXXXX

# 5. うまくいかなかったら、ログを見る／渡す
qrsend log --copy

# 6. ディスクが足りなくなったら
qrsend storage
qrsend storage clean
```

---

## `qrsend send`

```
qrsend send [OPTIONS] [PATH]...
```

ファイル・フォルダ・テキストを梱包（圧縮・暗号化・署名）して、QR コードのストリームとして流し続ける。受信側が何を読み落としても、十分な数のコードを読めば復元できる（噴水符号）ので、送信側は止められるまで流し続ける。

`PATH` はファイルかフォルダ（複数可）。`-` は標準入力を 1 つのファイルとして送る。

**暗号化は必須の選択**: `--to` で宛先を指定するか、`--plain` で暗号化しないことを明示する。どちらも無いとエラーになる。

### 何を送るか

| オプション | 説明 |
|---|---|
| `--text <TEXT>` | ファイルの代わりに、このテキストを送る |
| `--name <NAME>` | 標準入力（`-`）に付けるファイル名。既定は `stdin` |

### 宛先と暗号化

| オプション | 説明 |
|---|---|
| `--to <DEVICE>` | この信頼済みデバイス宛てに暗号化する。名前・フィンガープリント・`qrsend-id:…` のどれでも指定できる。繰り返すと複数の宛先になる。`--plain` とは併用できない |
| `--plain` | 暗号化せずに送る。コードが見える人は誰でも読める |
| `--sender-name <SENDER_NAME>` | 受信側に表示する送信者名。既定はこのデバイスの名前 |

### 表示

| オプション | 説明 |
|---|---|
| `--display <DISPLAY>` | コードを出す場所。`window`（既定。専用のウィンドウ。コードが大きく、最も速い）か `terminal`（この端末の中に文字で描く）。ウィンドウ無しのビルド（Linux の musl 版など）では `--display terminal` を指定する |
| `--fps <FPS>` | 1 秒あたりの枚数。既定は 10（`--dense` の動画は 30） |
| `--grid <GRID>` | 一度に並べるコードの数。`N`（N×N）、`COLSxROWS`（最大 64×64）、`auto`（ウィンドウや画像いっぱいに敷き詰める）。既定は `1`。ウィンドウと書き出しのときだけ |

ウィンドウ・端末の表示中のキー:

| キー | 動作 |
|---|---|
| `Space` | 一時停止／再開 |
| `↑` または `+`（`=`） | 速くする（1.25 倍、最大 60 枚/秒） |
| `↓` または `-` | 遅くする（最小 0.5 枚/秒） |
| `Esc` または `Q` | 終了（端末表示では `Ctrl-C` も） |

macOS でウィンドウ表示にすると、Dock に QRSend のアイコンが出る（ウィンドウを持つプログラムは Dock に並ぶ。送信が終わると消える）。

### コードの大きさ

| オプション | 説明 |
|---|---|
| `--density <DENSITY>` | QR の密度。`auto`（既定。小さな転送には小さく読みやすいコード、`--dense` の書き出しには最もよく載る大きさ）、`low`、`normal`、`high`、`max` |
| `--qr-version <QR_VERSION>` | QR のバージョン（1〜40）を直接指定する。`--density` より優先 |
| `--ecc <ECC>` | QR の誤り訂正レベル（`L`、`M`、`Q`、`H`）を直接指定する。`--density` より優先 |
| `--redundancy <REDUNDANCY>` | 1 パスごとに足す修復コードの割合（各セグメントに対する比）。既定は `0.1` |

### 圧縮

| オプション | 説明 |
|---|---|
| `--level <LEVEL>` | zstd の圧縮レベル（1〜22）。既定は `19` |
| `--no-compress` | 圧縮しない（すでに圧縮されているものを速く梱包したいとき） |

### 表示する代わりに書き出す

| オプション | 説明 |
|---|---|
| `--export-video <FILE>` | 動画に書き出す。`.y4m`（無圧縮。追加のツール不要）か、`.mp4` / `.mkv` / `.mov` / `.webm`（ffmpeg が必要）。どこかの画面で再生して撮影・キャプチャするか、`qrsend recv --video` で直接読む |
| `--export-frames <DIR>` | PNG の連番画像をこのディレクトリに書き出す |
| `--export-text <FILE>` | フレームを 1 行 1 個のテキストで書き出す（`-` は標準出力）。シリアル回線・TCP・ssh・ファイルなど、バイト列を運べるもの向け。`qrsend recv --text` で読む。QR の大きさを指定しなければ 1 フレーム 1024 バイト |
| `--dense` | 書き出しを最大の密度にする: 1 枚に載るだけコードを敷き詰める（`--grid auto --size 1920x1080 --scale 2`、動画は 30 枚/秒と同じ）。書き出し専用（ウィンドウでは `--grid auto`） |
| `--size <WxH>` | 書き出す画像の大きさ（ピクセル）。例: `1920x1080`、`3840x2160` |
| `--scale <PX>` | QR の 1 モジュールのピクセル数（書き出し時）。ウィンドウでは、`--grid auto` が使う最小の大きさ。既定は 4（`--dense` は 2） |
| `--frames <N>` | 書き出す枚数。既定は `--passes` 回ぶん |
| `--passes <N>` | 書き出すパス数。既定は `1`。2 パス目以降は新しい修復コードになるので、再生を撮影して取りこぼしが出るときに効く |
| `--video-crf <CRF>` | ffmpeg の画質（CRF。0 は無劣化、大きいほど小さい）。既定は `12` |

### 送り直しと、残すかどうか

送ったものは梱包した状態でキャッシュに 2 週間残り、梱包し直さずに送り直せる（[何がどこに残るか](#何がどこに残るか)）。

| オプション | 説明 |
|---|---|
| `--resume <CODE>` | 受信側の resume code（`QSR1-…`）に挙がっているセグメントだけを送り直す |
| `--session <ID>` | 以前の送信（セッション ID）をキャッシュから送り直す |
| `--no-keep` | 送信が終わったら、梱包したものを残さない（正常終了・エラー・`Ctrl-C` のどれでも消える）。そのぶん `--session` / `--resume` では送り直せず、途中で止まった受信側は最初からになる。環境変数 `QRSEND_NO_KEEP=1` で常にこの動作にできる |

### ネットワークでの直結

| オプション | 説明 |
|---|---|
| `--lan` | ローカルネットワークでの直接接続も申し出る。ストリームに「どこへ接続すればよいか」を伝えるコードが混ざり、受信側の `qrsend`（やブラウザ版）がそれを読むと自分から接続して、以後は接続でも運ぶ（カメラよりはるかに速い。コードの鍵で暗号化される）。受信側が全部受け取ると送信は自動で終わる |
| `--lan-address <ADDRESS>` | `--lan` で知らせるアドレスを指定する（複数のネットワークにつながっている機械向け） |
| `--lan-port <PORT>` | `--lan` で待ち受けるポート。既定は `0`（空いているどれか） |

### ログ

| オプション | 説明 |
|---|---|
| `--log <FILE>` | 診断ログを、`qrsend log` 用に残す代わりに FILE へ書く（`-` は標準エラー）。こちらは行数の上限なし。ファイル名・内容・デバイス ID・ネットワークのアドレスは記録されない |

### 例

```bash
qrsend send photos/ notes.md --to phone
qrsend send --text "hello" --to phone --to tablet
tar c project | qrsend send - --name project.tar --plain
qrsend send big.iso --plain --grid auto --fps 15          # ウィンドウいっぱいに並べる
qrsend send big.iso --plain --display terminal            # 端末の中に描く
qrsend send big.iso --dense --export-video stream.mp4 --plain
qrsend send big.iso --plain --lan                         # 受信側が自分から接続する
qrsend send big.iso --plain --no-keep                     # 送ったものをディスクに残さない
```

---

## `qrsend recv`

```
qrsend recv [OPTIONS]
```

ストリームを読んで受け取る。入力を 1 つ選ぶ（`--camera`、`--video`、`--images`、`--text` のどれか。無いとエラー）。

読んだものは inbox に貯まり、全部そろうと検証・復号・展開して `--out` に保存する。そろわないまま入力が終わると、**終了コード 2** で終わり、resume code を表示する（途中までの分は inbox に残るので、次の `recv` は続きから）。

### 入力

| オプション | 説明 |
|---|---|
| `--camera [<DEVICE>]` | カメラから読む（ffmpeg が必要）。デバイスは省略可。macOS は番号（`ffmpeg -f avfoundation -list_devices true -i ""` で一覧）、Linux は `/dev/videoN`、Windows はデバイス名 |
| `--camera-size <WxH>` | カメラに求める画像の大きさ。例: `1280x720`。指定しないとカメラの既定になり、小さいことが多い（Linux は 640x480 が多い）。密なコードにはもっと必要 |
| `--camera-fps <N>` | カメラに求める 1 秒あたりの枚数 |
| `--video <FILE>` | 動画から読む（`.y4m` はそのまま。他の形式は ffmpeg 経由） |
| `--images <PATH>...` | 画像ファイル、または画像の入ったディレクトリから読む（PNG / JPEG） |
| `--text <FILE>` | `qrsend send --export-text` が書いたテキスト（1 行 1 フレーム）を読む（`-` は標準入力） |

### 保存

| オプション | 説明 |
|---|---|
| `-o`, `--out <DIR>` | 保存先のディレクトリ。既定は `.`（今いる場所） |
| `--stdout` | 受け取った 1 つのファイル（またはテキスト）を標準出力に書く |
| `--copy` | 受け取ったテキストをクリップボードにコピーする |
| `--on-conflict <ON_CONFLICT>` | 同じ名前がすでにあるとき: `rename`（既定。両方残す。`name (1).ext` として保存）、`overwrite`（置き換える。ディレクトリは統合）、`skip`（今あるほうを残し、受け取ったほうを捨てる） |
| `--keep` | 保存したあとも inbox に残す（既定では保存と同時に inbox から消える） |

### そのほか

| オプション | 説明 |
|---|---|
| `--session <ID>` | このセッション（16 進 8 桁）だけを受け付ける |
| `--no-lan` | 送信側がネットワーク接続を申し出ていても（`qrsend send --lan`）接続せず、コードだけを読む |
| `--threads <THREADS>` | デコードに使うスレッド数。既定は全コア |
| `--log <FILE>` | 診断ログを、`qrsend log` 用に残す代わりに FILE へ書く（`-` は標準エラー）。行数の上限なし |

### 例

```bash
qrsend recv --camera -o ~/Downloads
qrsend recv --camera 1 --camera-size 1920x1080 --camera-fps 30
qrsend recv --video recording.mp4 -o ~/Downloads
qrsend recv --images frames/ --stdout > received.bin
nc -l 9000 | qrsend recv --text - -o ~/Downloads
```

---

## `qrsend id`

```
qrsend id [OPTIONS]
```

このデバイスの名前・フィンガープリント・ID（`qrsend-id:1:…`）と、ID の QR コードを表示する。初めて実行したときに鍵（X25519 と Ed25519）を作る。ID は公開してよい情報で、相手はこれを `qrsend devices add` で登録すると、このデバイス宛てに暗号化でき、このデバイスの署名を確かめられる。

| オプション | 説明 |
|---|---|
| `--name <NAME>` | このデバイスの名前（鍵を初めて作るときに使う） |
| `--rename <NAME>` | このデバイスの名前を変える |
| `--no-qr` | ID の QR コードを描かない |
| `--png <FILE>` | ID の QR コードを PNG に書き出す |

---

## `qrsend devices`

```
qrsend devices <COMMAND>
```

信頼済みデバイス（Trusted devices）の管理。登録した相手には `send --to` で暗号化して送れ、受け取ったときには送信者として確認できる（`From: laptop ✓`）。

### `qrsend devices list`

登録済みのデバイスを一覧する。

### `qrsend devices add`

```
qrsend devices add [OPTIONS] [ID]
```

デバイスを信頼する。`ID` は相手の `qrsend id` が表示したもの（`qrsend-id:1:…`）。登録の前にフィンガープリントを表示して確認を求めるので、相手の画面と同じであることを確かめる。

| オプション | 説明 |
|---|---|
| `--image <FILE>` | ID を、この画像の QR コードから読む（相手の画面のスクリーンショットなど） |
| `--name <NAME>` | この名前で登録する |
| `-y`, `--yes` | 確認を求めない |

### `qrsend devices rm`

```
qrsend devices rm <DEVICE>
```

登録を消す。`DEVICE` は名前かフィンガープリント。

### `qrsend devices rename`

```
qrsend devices rename <OLD> <NEW>
```

登録済みのデバイスの名前を変える。

---

## `qrsend inbox`

```
qrsend inbox <COMMAND>
```

受け取ったもの（受信セッション）の管理。途中で止まった受信は、消すまで inbox に残る。完了したものは保存と同時に消える（`recv --keep` を付けたとき・保存に失敗したときは残る）。`ID` は 16 進 8 桁のセッション ID（`inbox list` や `recv` が表示する）。

### `qrsend inbox list`

受信セッションを一覧する（ID、完了か途中か、中身の概要）。

### `qrsend inbox show`

```
qrsend inbox show <ID>
```

セッションの詳細とファイル一覧を表示する。

### `qrsend inbox export`

```
qrsend inbox export [OPTIONS] <ID>
```

完了したセッションのファイルを保存する。

| オプション | 説明 |
|---|---|
| `-o`, `--out <DIR>` | 保存先。既定は `.` |
| `--on-conflict <ON_CONFLICT>` | 同じ名前があるとき: `rename`（既定）、`overwrite`、`skip`（`recv` と同じ） |
| `--stdout` | 1 つのファイル（またはテキスト）を標準出力に書く |
| `--copy` | 受け取ったテキストをクリップボードにコピーする |
| `--remove` | 保存したあと inbox から消す |

### `qrsend inbox missing`

```
qrsend inbox missing <ID>
```

足りないセグメントを挙げた resume code（`QSR1-…`）を表示する。送信側で `qrsend send --resume <CODE>` に渡す。

### `qrsend inbox rm`

```
qrsend inbox rm <ID>
```

セッションを消す（途中まで受け取った分も消える）。

---

## `qrsend cache`

```
qrsend cache <COMMAND>
```

送り直し用に残している送信データ（`send` が梱包したもの。2 週間残る）の管理。容量も含めて見たいとき・一部だけ消したいときは [`qrsend storage`](#qrsend-storage) のほうが向いている。

### `qrsend cache list`

残している送信データを一覧する（セッション ID、大きさ、中身の概要）。ID は `send --session` に渡せる。

### `qrsend cache clean`

残している送信データを **すべて** 消す（確認は求めない）。

---

## `qrsend storage`

```
qrsend storage
qrsend storage <COMMAND>
```

QRSend がこの機械に残しているもの全部と、それぞれがディスク上で使っている大きさを表示する。対象は 3 種類: 受け取ったもの（inbox）、送ったもの（送り直し用）、ログ。デバイス ID と信頼済みデバイスは対象外で、このコマンドでは決して消えない。

```
QRSend keeps 48.00 KiB on this machine.

Received (the inbox) — 12.00 KiB
  in /Users/me/Library/Application Support/qrsend/sessions
   1  1ed709b7   12.00 KiB  just now     complete · demo — 1 file, 6 B

Sent (kept two weeks, to be sent again) — 12.00 KiB
  in /Users/me/Library/Caches/qrsend/send
   2  1ed709b7   12.00 KiB  just now     demo (1 file, 6 B) · removed in 14 days

Logs of the last runs — 24.00 KiB
  in /Users/me/Library/Caches/qrsend/logs
   3  logs       24.00 KiB  just now     6 logs

To make room: qrsend storage clean
```

### `qrsend storage list`

一覧を表示する（`qrsend storage` だけのときと同じ）。

### `qrsend storage clean`

```
qrsend storage clean [OPTIONS] [ID]...
```

残しているものを消す。**オプション無しなら、一覧を出して、どれを消すかを番号で尋ねる**（`1 3-5` のように複数・範囲で、または `all`）。消す前に確認する。

オプションで指定することもできる。種類・ID・古さを組み合わせると、すべてに当てはまるものだけが対象になる。端末から実行していないとき（スクリプトなど）は、対象の指定と `--yes` が無ければ何も消さない。

| オプション | 説明 |
|---|---|
| `[ID]...` | これらだけを消す（`qrsend storage` が表示する ID） |
| `--all` | 一覧にあるものすべて（デバイス ID と信頼済みデバイスは含まない） |
| `--inbox` | 受け取ったもの（inbox） |
| `--sent` | 送り直し用に残している送信データ |
| `--logs` | 過去の実行のログ |
| `--older-than <AGE>` | 最後に書かれてからこれ以上たったものだけ。`7d`、`12h`、`30m`（数字だけなら日数）。単独でも、ほかの指定を絞るのにも使える |
| `-y`, `--yes` | 確認せずに消す |
| `--dry-run` | 何が消えるかを表示するだけで、消さない |

```bash
qrsend storage clean                        # 一覧から選ぶ
qrsend storage clean --sent                 # 送ったものを全部
qrsend storage clean --older-than 7d        # 1 週間以上前のものを全部
qrsend storage clean --inbox --older-than 30d --dry-run
qrsend storage clean 1ed709b7 -y
```

---

## `qrsend log`

```
qrsend log [OPTIONS] [RUN]
qrsend log <COMMAND>
```

`send` / `recv` の診断ログを表示する。ログは頼まなくても毎回残り（直近 10 回分）、何がいつ起きたか（毎秒の画像数・コード数、見つけた転送、試した接続とその経路の種類など）が 1 行ずつ入っている。**ファイル名・内容・デバイス ID・ネットワークのアドレスは記録されない** ので、不具合の報告にそのまま貼れる。

`RUN` は「どの回か」で、git のコミットと同じ数え方をする: `HEAD`（既定。直近）、`HEAD~1` または `HEAD^`（1 つ前）、`HEAD~2`（2 つ前）…。`@` は `HEAD` と同じ。シェルによっては `^` を別の意味に取るので、`HEAD~1` の書き方が確実。

| オプション | 説明 |
|---|---|
| `--copy` | 表示する代わりにクリップボードへコピーする |

残すログは 1 つ最大 2000 行（長い実行は最初と最後を残す）で、本人しか読めない。`send` / `recv` の `--log FILE`（または環境変数 `QRSEND_LOG=FILE`）を使うと、その回は残さずに FILE へ全行を書く。`QRSEND_LOG=off` なら何も残さない。

### `qrsend log show`

```
qrsend log show [OPTIONS] [RUN]
```

ログを表示する（`qrsend log` だけのときと同じ。`RUN` と `--copy` も同じ）。

### `qrsend log list`

残しているログを一覧する。左端が、そのログを指す書き方。

```
HEAD     2026-10-12 02:05:17 UTC  recv    4.00 KiB  just now
HEAD~1   2026-10-12 02:05:17 UTC  send    4.00 KiB  just now
HEAD~2   2026-10-12 02:04:28 UTC  send    4.00 KiB  1 min ago
```

### `qrsend log clear`

残しているログをすべて消す。

---

## `qrsend completions`

```
qrsend completions <SHELL>
```

シェルの補完スクリプトを標準出力に書く。`SHELL` は `bash`、`elvish`、`fish`、`powershell`、`zsh`。

```bash
qrsend completions zsh > ~/.zfunc/_qrsend         # zsh（~/.zfunc が fpath にある場合）
qrsend completions bash > ~/.local/share/bash-completion/completions/qrsend
qrsend completions fish > ~/.config/fish/completions/qrsend.fish
```

---

## 環境変数

| 変数 | 意味 |
|---|---|
| `QRSEND_DATA_DIR` | データ（inbox）の置き場を変える |
| `QRSEND_CACHE_DIR` | キャッシュ（送り直し用の送信データ、ログ）の置き場を変える |
| `QRSEND_CONFIG_DIR` | 設定（デバイス ID、信頼済みデバイス）の置き場を変える |
| `QRSEND_NO_KEEP` | `1` にすると、`send` が常に `--no-keep` で動く |
| `QRSEND_LOG` | ログの行き先。ファイルのパス（その回は残さず、そこへ全行を書く）、`-`（標準エラー）、`off`（何も残さない）。`--log` があればそちらが優先 |
| `QRSEND_TRACE` | 設定すると、ネットワーク接続の経過を標準エラーに出す（`[link] …`。**アドレスを含む** ので、人に渡すなら `qrsend log` のほうを使う） |

## 終了コード

| コード | 意味 |
|---|---|
| `0` | 成功 |
| `1` | エラー（メッセージを標準エラーに出す。`send` / `recv` なら「A log of this run was kept」と案内する） |
| `2` | `recv` が、転送を全部は受け取れないまま終わった（途中までの分は inbox に残り、resume code を表示する） |

`send` を `Ctrl-C` で止めたときは、シグナル（SIGINT）で終わったことになる。

## 何がどこに残るか

| 残すもの | 場所 | いつまで |
|---|---|---|
| 受け取ったもの（inbox）。暗号化・圧縮されたまま | データディレクトリの `sessions/` | 途中のものは、完了するか消すまで。完了したものは保存と同時に消える（`--keep` を付けたとき・保存に失敗したときは残る） |
| 送ったもの。梱包済みで、送り直せる | キャッシュディレクトリの `send/` | 2 週間（期限切れは次の `send` のときに消える）。`send --no-keep` なら残さない |
| ログ | キャッシュディレクトリの `logs/` | 直近 10 回分。1 つ数キロバイト |
| デバイス ID と信頼済みデバイス | 設定ディレクトリ | 自分で消すまで（`qrsend storage` は消さない） |

| | データ | キャッシュ | 設定 |
|---|---|---|---|
| macOS | `~/Library/Application Support/qrsend` | `~/Library/Caches/qrsend` | `~/Library/Application Support/qrsend` |
| Linux | `~/.local/share/qrsend` | `~/.cache/qrsend` | `~/.config/qrsend` |
| Windows | `%LOCALAPPDATA%\qrsend` | `%LOCALAPPDATA%\qrsend` | `%APPDATA%\qrsend` |

送ったものは「送ったものを圧縮した大きさ」をそのまま使う。大きなものを送ったあとは `qrsend storage clean --sent` ですぐに空けられる。

## 外部のツール

- **ffmpeg**: `recv --camera`、`.y4m` 以外の動画の読み書き（`recv --video`、`send --export-video`）に必要。無くても、ウィンドウ・端末での表示、`.y4m`、画像、テキストは使える。

---

QR コードは株式会社デンソーウェーブの登録商標です。QR Code is a registered trademark of DENSO WAVE INCORPORATED in Japan and in other countries.
