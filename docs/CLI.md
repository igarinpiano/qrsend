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

### 一番厳しい送り方（`--secure`）

| オプション | 説明 |
|---|---|
| `--secure` | 一番厳しい条件で送る。下の 3 つを守り、それに反するオプションは **エラー** にする（黙って例外にはしない）。環境変数 `QRSEND_SECURE=1` で常にこの動作にできる |

0. **前方秘匿性つきでだけ送る。** 受信側の使い捨ての鍵（`--one-time-key`）が必須。受信側が転送のあとでそれを消すので、後からデバイスの鍵が漏れても、録画されたコードは読めない。
1. **暗号化・署名したものを、信頼済みデバイスにだけ送る。** `--to` が必須で、`--plain` はエラー。`--to qrsend-id:…` と ID を直接書いた場合も、信頼済みデバイス（`qrsend devices`）に登録されていなければエラー。`--session` / `--resume` で以前の梱包を送り直すときも、暗号化されていなければエラー。
2. **一方向だけ。外から受け取るのは、受信側の使い捨ての鍵だけ。** ネットワークの接続を申し出ない・待ち受けない（`--lan` はエラー）。受信側からのフィードバックも読まない（`--two-way`・`--feedback-from` はエラー。フィードバックは署名の無い情報で、カメラも開くため）。データは画面のコードだけを通る。
3. **この機械に何も残さない。** 梱包したものを残さない（`--no-keep` と同じ）。ログも残さない（`--log` はエラー。`QRSEND_LOG` も無視する）。

書き出し（`--export-frames` など）は使える（書き出されるのは暗号化済みのコードで、置き場所は利用者が決めたもの）。`recv` の `--secure` は [`qrsend recv`](#qrsend-recv) の項。

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

### カラーコード

| オプション | 説明 |
|---|---|
| `--color` | 1 つの枠に 3 個のコードを赤・緑・青に重ねて表示する（Web 版の「Color codes」）。色を読み分けられるカメラなら 1 枚で 3 倍運べる。読み分けられないカメラには 3 個のうち 1 個（たいてい緑）だけが読めるか、まったく読めない。返事をくれる受信側（`--two-way`）がいれば、報告に合わせて読めない色を外し、同じに見える色をまとめ、区別できなければ白黒に戻す（返事が 8 秒来ない間と、返事はあるのに 6 秒読めていないときも白黒）。一方向では送信側には分からないので、読めなければ `--color` を外す。ウィンドウと `--export-frames` のみ（動画・端末の表示・テキストには使えない）。`qrsend recv` とブラウザ版は、カラーかどうかを自動で見分けて読む |

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
| `--frames <N>` | 書き出す枚数。既定は `--passes` 回ぶん（`--export-text` に `--feedback-from` を付けたときは、どちらも指定しなければ「受信側が全部受け取るまで」） |
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

**つながらないとき（ファイアウォール）**: 接続するのは受信側で、送信側は待つだけなので、送信側の機械のファイアウォールが受信側を追い返すと、どちらにも何も知らされないまま、いつまでもつながらない。

- macOS では、`--lan` の開始時にファイアウォールの設定を調べる。`qrsend` への外からの接続が通らない設定なら、端末で「Allow them now? … [y/N]」と尋ね、`y` と答えると、その場でファイアウォールの設定を変える（macOS 付属の `socketfilterfw` を `sudo` で実行するので、管理者のパスワードを尋ねられる）。端末でないとき・断ったときは、手で直す方法（システム設定 → ネットワーク → ファイアウォール → オプション、または同じコマンド）を表示する。ログには `firewall incoming=blocked` / `asked` / `open` と残る。
- **何もしなくても通るようにはできない**: ファイアウォールの設定を変えるには管理者の権限が要る。インストールした時点で通るアプリ（LocalSend など）は、Apple の Developer ID で署名されていて、ファイアウォールの「署名付きのソフトウェアを自動的に許可する」設定（Automatically allow downloaded signed software to receive incoming connections）の対象になっている。`qrsend` の配布物は現在その署名をしていない。
- 許可は実行ファイルの置き場所ごと。mise や nvm のように Node のバージョンごとに置き場所が変わる入れ方では、バージョンを上げるたびに改めて必要になる。
- Windows・Linux では調べる手段が無いので、つながらないときはファイアウォールを確かめる。
- ブラウザ版の受信側は、申し出に応じても送信側に届かなかったとき、その旨を表示する。

### 前方秘匿性（使い捨ての鍵）

受信側が転送の前に **使い捨ての鍵** を見せ、送信側はそれも使って暗号化する。受信側は、転送が届いて保存した時点でその鍵を消す。以後は、コードを録画していた人が **受信側のデバイスの鍵を後から手に入れても、読めない**（[docs/POST-QUANTUM.md](POST-QUANTUM.md) のフェーズ 6、PROTOCOL §9.2）。

| オプション | 説明 |
|---|---|
| `--one-time-key <KEY>` | 受信側が見せた使い捨ての鍵。`KEY` は文字列（`QSK1-…`。受信側の `qrsend recv --one-time-key` やブラウザ版の Receive が表示する）か、`camera`（`camera:DEVICE`）で受信側の画面のコードをカメラから読む。宛先ごとに 1 つ（`--to` が必要）。鍵は「どのデバイスのものか」を持っていて、宛先のどれにも合わなければエラー。耐量子の鍵を持つ宛先にだけ使える |

- 使い捨ての鍵は署名されていない（必要が無い）: ファイルの鍵を開けるには **宛先のデバイスの鍵も** 要るので、他人が自分の鍵にすり替えても何も読めない（転送が失敗するだけ）。
- 受信側が鍵を消すのは、保存が終わったとき（`--keep` で inbox に残した場合は消さない）。誰にも使われなかった鍵と、途中で止まった転送の鍵は、**1 日** で消える（途中で止まった転送は、それまでに続きを受け取る）。
- `--secure` では **必須**（無ければエラー）。

### 双方向（Two-way）

受信側から「まだ足りないもの」を返してもらい、足りない分だけを送って、全部届いたら自動で終わる。返事が途絶えても転送は止まらない（2 秒で返事待ちをやめ、10 秒で「何も知らない」状態に戻って全部を流す）。返事は署名の無い助言で、何を次に送るかを決めるだけ（届いたことの証明には使わない）。

| オプション | 説明 |
|---|---|
| `--two-way` | 受信側の画面に出るフィードバックコードを、こちらのカメラ（ffmpeg 経由）で読む。ストリームに「フィードバックを読みます」という合図が混ざり、ブラウザ版の受信側はそれを見るとフィードバックコードを表示する。`qrsend recv` は `--two-way` を付けるとウィンドウに表示する |
| `--auto-speed` | 表示の自動調節（Web 版の「Automatic speed」）。返事をくれる受信側（`--two-way` か `--feedback-from` が必要）の報告を見て、1 秒あたりの枚数と 1 枚あたりのコード数を決める。変更を 1 つずつ試し、届く量が増えれば採用、増えなければ戻す。受信側がカメラの様子（毎秒の読み取り回数、1 ドットが何ピクセルに写っているか）を報告していれば、最初の 1 回はそれを使って一気に近づく。`--fps` と 1 個から始め、ウィンドウに `--scale` で収まる配置まで増やす（ウィンドウの大きさを変えれば選択肢も変わる）。`--grid COLSxROWS` を指定すると配置は固定で、速さだけを調節する。ウィンドウのみ。ネットワーク接続で運んでいる間は調節しない |
| `--two-way-camera <DEVICE>` | `--two-way` で使うカメラ。`qrsend recv --camera` と同じ書き方（macOS は番号、Linux は `/dev/videoN`、Windows はデバイス名） |
| `--feedback-from <FILE>` | フィードバックコードをテキスト（1 行 1 個）として FILE から読む（`-` は標準入力）。`qrsend recv --feedback-to` が書くもの。シリアル回線や ssh など、バイト列を運べる経路での双方向。`--export-text` と一緒に使うと、枚数を指定しない限り、受信側が全部受け取るまでフレームを書き続ける。1 パスぶん書くごとに、受信側の返事を少しだけ待つ（最初の返事は最大 4 秒、以後は 0.2 秒。パイプのような速い経路では、返事が戻るよりずっと速く書けてしまうため）。返事が一度も来なければ 3 パスぶんで止まる |

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
qrsend send secret.pdf --to phone --secure                # 一番厳しく: 暗号化のみ・一方向のみ・何も残さない
qrsend send big.iso --plain --two-way                     # 受信側の返事をカメラで読み、足りない分だけ送る
qrsend send big.iso --plain --two-way --auto-speed        # さらに、速さとコード数を返事から自動で決める
qrsend send big.iso --plain --color                       # 赤・緑・青に 3 個重ねる
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

### 一番厳しい受け取り方（`--secure`）

| オプション | 説明 |
|---|---|
| `--secure` | 一番厳しい条件で受け取る。下の 3 つを守り、それに反するオプションはエラーにする。環境変数 `QRSEND_SECURE=1` で常にこの動作にできる |

0. **前方秘匿性つきの転送だけを受け取る。** 始めに使い捨ての鍵を作って表示し（`--one-time-key` と同じ）、その鍵宛てに暗号化されていない転送は断る。
1. **この端末宛てに暗号化され、信頼済みデバイスが署名した転送だけを受け取る。** 暗号化されていない・署名が無い・署名者が信頼済みでない転送は、エラーで断り、途中まで貯めた分も inbox から消す。
2. **一方向だけ。外へ出すのは、使い捨ての鍵だけ。** ネットワークには接続しない（`--no-lan` と同じ）。送信側へほかに何も返さない（`--two-way`・`--feedback-to` はエラー）。
3. **保存したもの以外を残さない。** ログを残さない（`--log` はエラー）。inbox に何も残さない: 保存したら消し（`--keep` はエラー）、**途中で止まった転送も、終了時に消す**（resume code は出ない。次は最初から受け取る）。クリップボードに入れない（`--copy` はエラー。クリップボードはどのプログラムからも読める）。

途中で止まった転送を消すのは、残しても続きを受け取れるとは限らないため: 続きには送信側が同じ梱包を持っている必要があり、送信側も `--secure`（または `--no-keep`）なら、送信が終わった時点で梱包は消えている（ファイルを送り直すと、別の鍵で暗号化された別の転送になる）。送信側がまだ流している間なら、`recv --secure` をやり直せば最初から受け取れる。

### 前方秘匿性（使い捨ての鍵）

| オプション | 説明 |
|---|---|
| `--one-time-key` | 読み始める前に、使い捨ての鍵を作って表示する: 文字列（`QSK1-…`、約 1,850 文字）と、端末なら Enter で QR コード（3〜4 個。`qrsend id` と同じ操作: `space` で止める、`←` `→` で 1 個ずつ、`q` で受信へ進む）。送信側はそれを `qrsend send --one-time-key` かブラウザ版の Send で読む。届いて保存したら鍵を消す（「The one-time key of this transfer is destroyed.」）。説明は [`qrsend send`](#qrsend-send) の「前方秘匿性」 |

### 双方向（Two-way）

| オプション | 説明 |
|---|---|
| `--two-way` | 送信側のカメラに見せるフィードバックコード（まだ足りないもの）を、専用のウィンドウに表示する。送信側（ブラウザ版の「Two-way transfer」、または `qrsend send --two-way`）はそれを読んで足りない分だけを送り、全部届いたら自動で止まる。保存が終わると「全部届いた」を数秒表示してから終わる。ウィンドウを閉じると、以後は一方向で受け取り続ける。ウィンドウ無しのビルドでは使えない（`--feedback-to` は使える） |
| `--feedback-to <FILE>` | 同じ返事をテキスト（1 行 1 個）で FILE に書く（`-` は標準出力）。送信側は `qrsend send --feedback-from` で読む。シリアル回線や ssh など向け。変化があれば 1 秒に 10 回まで、無くても 1 秒に 1 回。`--stdout` とは一緒に使えない |

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

このデバイスの名前・フィンガープリント・ID と、ID の QR コードを表示する。初めて実行したときに鍵を作る。ID は公開してよい情報で、相手はこれを `qrsend devices add` で登録すると、このデバイス宛てに暗号化でき、このデバイスの署名を確かめられる。

**鍵は 4 つ**（[docs/POST-QUANTUM.md](POST-QUANTUM.md)）: 従来の X25519（暗号化）と Ed25519（署名）に加えて、量子計算機でも破れない ML-KEM-768 と ML-DSA-65 を持ち、常に **両方を一緒に** 使う。以前の版で作った鍵には、初めて使うときに新しい 2 つが自動で足される（従来の鍵は変わらないので、登録済みの相手とはそのまま使える。相手にこのデバイスの ID を登録し直してもらった時点から、耐量子になる）。

- **ID**: `qrsend-id:2:…`（約 4,300 文字。公開鍵が 3.2 kB ある）。そのままコピーして `qrsend devices add` に貼れる。
- **フィンガープリント**: 20 桁（`1234-5678-9012-3456-7890`）。
- **どちらの鍵が使われているかは、いつも表示される**: `qrsend id` の `Keys:` 行、`qrsend devices list` の各行（`post-quantum keys` / `OLDER KEYS ONLY`）、`send` の `Encrypted for: … · post-quantum protection` と送信中のタイトル、`recv` の `From: 名前 ✓ (post-quantum signature)` と `Encrypted for this device, with post-quantum protection`。従来の鍵だけが使われているときは、送る側・受け取る側の両方に `WARNING:` が出る（従来の方式だけで暗号化される／署名を従来の方式だけで確かめた）。
- **QR コード**: 1 個には収まらないので、**9 個ほどに分けて順に表示する**（端末は 81 桁 × 42 行以上が必要）。Enter で表示を始め、`space` で止める／再開、`←` `→` で 1 個ずつ送る、`q` で終わる。読む側は、どの順でも、同じものを何度読んでもよい。

| オプション | 説明 |
|---|---|
| `--name <NAME>` | このデバイスの名前（鍵を初めて作るときに使う） |
| `--rename <NAME>` | このデバイスの名前を変える |
| `--no-qr` | ID の QR コードを描かない |
| `--png <FILE>` | ID の QR コードを PNG に書き出す。耐量子の鍵つきの ID は複数になり、`FILE-1.png`、`FILE-2.png` … と書く |
| `--classic` | 耐量子の鍵が入る前の版（0.1.10 以前）が読める形の ID を表示する（`qrsend-id:1:…`。短いコード 1 個、従来の鍵だけ）。この ID を登録した相手からの転送は、従来の方式だけで暗号化される |

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

デバイスを信頼する。`ID` は相手の `qrsend id` が表示したもの（`qrsend-id:2:…`、または従来の `qrsend-id:1:…`）。登録の前にフィンガープリントを表示して確認を求めるので、相手の画面と同じであることを確かめる。

従来の ID で登録済みの相手の、耐量子の鍵つきの ID を登録すると、新しく増えるのではなく、**その相手の記録に鍵が足される**（名前はそのまま）。以後、その相手には両方の方式で暗号化し、その相手の署名は両方そろっていなければ受け付けない。`qrsend devices list` は、従来の鍵だけの相手に「older keys only」と付ける。

| オプション | 説明 |
|---|---|
| `--image <FILE>...` | ID を、画像の QR コードから読む（相手の画面のスクリーンショットなど）。耐量子の鍵つきの ID は複数のコードなので、全部の画像を並べる（順番は問わない。1 枚に複数写っていてもよい） |
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
| `QRSEND_SECURE` | `1` にすると、`send` と `recv` が常に `--secure` で動く |
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
