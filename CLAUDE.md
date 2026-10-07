# QRSend — プロジェクトメモ（Claude 向け）

QR コードのストリームでテキスト・ファイル・フォルダを送るツール。CLI（Rust 単一バイナリ）とブラウザ版（GitHub Pages / PWA）の 2 本立て。

- 構想・決定事項: `docs/CONCEPT.md`
- ワイヤ形式の仕様: `docs/PROTOCOL.md`（**プロトコルに触れる変更は必ずここも更新する**）

## ユーザーについて

- やり取りは日本語。docs も日本語。**アプリの UI・CLI のメッセージ・README は英語のみ**。英語は**アメリカ式の綴り**（color、behavior、center、recognize、toward。コメントや識別子も同じ）。
- Rust 好き。Web フロントエンドには詳しくないので、Web 側の判断は具体例つきで説明する。
- 方針として決まっていること: 大容量優先（時間がかかるのは可、圧縮は手を抜かない）、暗号化は公開鍵方式（age ＋ 送信者署名、Trusted devices）、UI フレームワークは Svelte 5。
- 目指す理想形（ユーザーの言葉）: 一方的に送るだけでなく、転送前・転送中に受信側から必要な情報（密度・サイズ・位置・ピント・カメラ性能など）を送り返して自動調節する。QR 以外も含む複数の通信手段を同時に使い、最速の組み合わせを選ぶ。配置は正方形に限らない。設計変更のときは、逆方向チャネルと複数トランスポートの余地を塞がないこと。
- 商標表記: 利用者の目に触れる場所（README、Web のフッター、CLI の `--help`、docs の末尾）に「QR Code is a registered trademark of DENSO WAVE INCORPORATED in Japan and in other countries.」を入れている。新しい配布物や画面を作るときも入れる。

## 構成

```
crates/qrsend-core/  プロトコル本体（I/O は std::io トレイトのみ。WASM と共有する）
  base45 / frame      フレーム形式（ヘッダ 22B + シンボル + CRC32、Base45 で QR 英数字モード）
  fec                 RaptorQ（raptorq crate）。1 セグメント = 1 ソースブロック、ESI は RFC 6330 準拠
  schedule / sender   参考送信スケジュール（窓インターリーブ、Meta は毎パス＋差し込み）。フィードバックがあれば受信済みを送らず、窓が受信されるまで次へ進まない（Sender::apply_feedback / forget_receiver）
  receiver            受信状態機械（暗号・I/O なし。Event を返す）。直近 8192 フレームの重複を捨てる（カメラは同じコードを何度も見る）。1 セグメントは 1 つのシンボルサイズで集め、別サイズが「より多く運んできた」ら乗り換える（複数経路の同時利用、PROTOCOL §13）
  feedback            受信側 → 送信側のフィードバックコード（QSF1-…）と、送信側からの合図（notice、QSC1-…）。PROTOCOL §11
  sound               音の変復調（1 シンボル = 0.04 秒、五音音階の 2 声を 1 音ずつ、オルゴール風に減衰。ユーザーの要望: 聞いて不快でないこと）。フィードバックを送信側のマイクへ返す経路。PROTOCOL §14
  link                別の経路を張るためのリンクコード（QSL1-…、分割と組み立ての枠、Assembler、TCP offer の中身）。PROTOCOL §12
  direct              失われない経路（ネットワーク接続）用の送信器 DirectSender（ソースシンボルを 1 回ずつ。欠けが確定した分だけ修復シンボル）と、バイナリのレコード詰め（pack / unpack / Record）。PROTOCOL §12.1
  tune                フィードバックの「読めたコード数」から、枚数/秒とコード数/枚を山登りで決める Tuner。PROTOCOL §11.4
  manifest            manifest JSON とメタエンベロープ（"QSM"、署名枠つき）
  payload             ファイル内容の単純連結＋zstd、BLAKE3 検証、展開（UnpackSink）。Packer は pull（add_file）と push（begin_file / write_chunk / end_file）の両方
  compress            zstd の切替（zstd-native / ruzstd）。ruzstd は 4MiB ごとの複数フレーム
  crypto              デバイス ID、age 暗号化、署名。復号は &dyn age::Identity を受ける。SharedSecrets は外部（WebCrypto）で計算した X25519 共有秘密で復号する identity
  sanitize / resume   パス無害化、resume code（QSR1-…）
  qr                  QR 生成（fast_qr・英数字 1 セグメント・マスク固定）と検出（rqrr）。auto_params（小さな転送は小さなコード）、best_tiling（フレームに最も多く載るバージョンと格子）、QUIET=4（隣のコードと共有する余白）
crates/qrsend/       CLI（バイナリ名 qrsend）
  spool               送信データをキャッシュに固めたもの（resume 用に 14 日保持）
  store / recv        受信セッション（inbox）。暗号文のまま保存→完了後に展開
  display/            GridSpec（N / COLSxROWS / auto）、window（minifb, feature "window"）/ terminal（crossterm）/ export（PNG、動画: .y4m は自前、他は ffmpeg にパイプ）
  input / decode      画像・Y4M・ffmpeg 経由の動画。decode は「1 個見つける → 格子をたどって隣を切り出し個別にデコード」。動画では前フレームの格子を再利用
  identity / devices  デバイス ID（X25519+Ed25519）と Trusted devices（QRSEND_CONFIG_DIR）
  net                 CLI 同士の TCP 直結（send --lan）。待ち受け・接続・暗号化したレコード（ChaCha20-Poly1305）。PROTOCOL §12.2
crates/qrsend-wasm/  Web 用バインディング（wasm-bindgen）。コールバック方式: SendJob（本体を JS の write に書き出す）/ SendSession（JS の read からセグメントを読む）/ Receive（完成セグメントを JS に渡す。extract は read → sink へストリーミング）
web/                 Svelte 5 + Vite + TS の PWA
  src/lib/engine.worker.ts  エンジン本体（Worker）。wasm とストレージを持ち、送信の梱包・フレーム生成・受信状態・展開を担当。ページとは engine.ts の RPC でやり取り
  src/lib/storage.ts        OPFS の同期アクセスハンドル（無ければメモリ）。受信は recv/<session>/{body,meta,out}、送信は send/<uuid>/body
  src/lib/keys.ts           デバイス ID。WebCrypto の non-extractable 鍵（X25519 / Ed25519）。非対応ブラウザは legacy（wasm 内の鍵）。旧形式（文字列）は初回に自動移行
  src/lib/                  core.ts（wasm 読み込み）/ db.ts（IndexedDB: 鍵・信頼デバイス・セッション一覧）/ scanner.ts + scan.worker.ts（BarcodeDetector → zxing-wasm、カメラと動画ファイル）/ save.ts + zip.ts（ディスクから直接保存、ZIP64 対応の無圧縮 ZIP）/ qrdraw.ts
  src/lib/guide.ts          受信カメラの持ち方の案内（preview「Camera guidance」）。純粋なロジックで、e2e/guide.spec.ts が Node 上で直接テストする
  src/views/                Home / Send(+Player) / Receive(+Camera, Result) / Devices / Inbox / Preview（Feature preview）
  e2e/                      Playwright。CLI が書いた Y4M を Chrome の仮想カメラに流す相互運用テスト（暗号化・多セグメント・ZIP を含む）。bridge.ts は 2 つのページを「互いのカメラ」として生でつなぐ（BroadcastChannel で画像を送り、getUserMedia を canvas.captureStream に差し替える）。相手の反応を見て変わる機能（自動調節、案内）はこれで試す
npm/                 npm 配布用: assemble.py（機種別パッケージの対応表）と launcher.js
```

## コマンド

```bash
cargo test --workspace                 # 全テスト（core の単体＋CLI の E2E）
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo build --release -p qrsend --no-default-features   # ウィンドウ無しビルド
```

Web:

```bash
cd web && npm ci
npm run wasm        # crates/qrsend-wasm をビルドし src/wasm/ に bindings を生成（wasm-bindgen-cli 0.2.129 が必要）
npm run dev         # 開発サーバ
npm run check && npm run build
cargo build --release -p qrsend && npx playwright test   # e2e（CLI バイナリを使う。動画ファイル受信のテストは ffmpeg が無いと skip）
```

手動 E2E（実カメラ無しで往復できる）:

```bash
export QRSEND_DATA_DIR=/tmp/q/data QRSEND_CACHE_DIR=/tmp/q/cache   # 本物の inbox/cache を汚さない
qrsend send some/dir --export-frames /tmp/q/f   # or --export-y4m v.y4m
qrsend recv --images /tmp/q/f -o /tmp/q/out
```

## 実装上の注意

- `cargo fmt` で整形されるため、スクリプトで文字列置換して編集すると一致しないことがある。編集は Read → Edit で行う。
- core は WASM でも使うので、スレッド・ファイルシステム・プロセスを直接使わない。zstd は feature で切替: `zstd-native`（既定、C ライブラリ）/ `ruzstd`（WASM、レベル 1 相当・finish 時に一括圧縮）。どちらも標準 zstd なので相互に読める。
- 乱数は rand 0.8 の OsRng（age と同じ getrandom 0.2 系に揃え、wasm では getrandom の `js` feature を qrsend-wasm 側で有効化）。
- CLI のデコード（decode.rs）: rxing は縮小・圧縮に強いが多数のコードは苦手、rqrr は位置（四隅・モジュール数）を返すがスクリーンショットに弱い。全体を一度に読むと 2px モジュールの 28 個入りフレームでどちらも半分程度しか読めなかったので、「rxing で 1 個 → rqrr で位置を測る → 4 モジュール間隔の格子をたどって各セルを切り出し個別にデコード（失敗時は 2〜3 倍に拡大して再試行）」にしている。実測: 12×12 で 144/144、4K・1px モジュール 299 個で 290。rqrr を密なフレーム全体にかけると数十秒かかるので避ける。
- QR 生成は fast_qr ＋マスク固定（1 個 0.9ms。qrcode crate のマスク選定は 11ms）。qrcode crate は容量計算と ID 用 QR（render_text）にだけ使う。
- clap で `Option<T>` 型の引数に独自の value_parser（Option を返す）を付けると実行時に型不一致で panic する。`--density` は専用 enum（DensityArg）にしている。
- raptorq は 32bit ARM で `std` feature を切っている（NEON の unstable intrinsics を使うため stable でビルドできない）。
- Web の wasm コールバックは同期。OPFS の同期ハンドルは「開く」のが非同期なので、展開先は 1 セッション 1 ファイル（out）にまとめ、各ファイルはその中の範囲（offset, size）として記録する。ページ側は Blob の slice で取り出す（メモリに載せない）。
- **WebKit（Safari、iPhone / iPad の全ブラウザ）は X25519 / Ed25519 の CryptoKey を IndexedDB に保存できない**: `put` はエラーなく成功するのに `get` が何も返さない（0.1.1 では「Create device ID を押しても無反応」になり、0.1.0 から移行した ID も失われた）。`keys.ts` の `canKeepKeys()` が使い捨ての鍵で保存→読み出し→利用を試し、だめなら legacy（wasm 内の鍵を文字列で保存）にする。本物の ID で試してはいけない。iPhone の Chrome も中身は WebKit なので、Mac の Chrome で動いても確認にならない → `web/e2e/webkit.spec.ts`（Playwright の WebKit）で確認する。
- Web の暗号化セッション: age ヘッダの X25519 スタンザごとに WebCrypto で共有秘密を計算して wasm に渡す（Meta と Body は別の age ファイルなので 2 回）。署名は SendJob.seal() が返すメッセージを WebCrypto で署名して finish() に渡す。
- OPFS はオリジン共有なので、送信スプールは Worker ごとの UUID ディレクトリに置く。
- QR は `qr::render` が常に単一の英数字セグメントで符号化する（汎用の最適化器だと固定バージョンで容量オーバーする実例があった）。容量は `QrParams::symbol_size()` で決まる。
- 受信側は frame の CRC → セッション整合 → RaptorQ → セグメント BLAKE3（manifest 到着後）→ 全体 BLAKE3 → ファイル BLAKE3 の順に検証する。manifest 到着前のセグメントは `unverified` として保存し、後で検証する。
- `recv` は未完了で終わると終了コード 2 と resume code を出す。
- **作りかけの機能は Feature preview に置く**（ユーザーの方針: 基本は従来の方式。新機能は送信側が `#/preview` で個別にオンにする）。定義は `web/src/lib/prefs.ts` の `PREVIEW_FEATURES`、保存先は localStorage の `qrsend.preview.<id>`。オフのときは画面も送る内容も従来と同じにする。受信側には設定を作らず、送信側からの合図で自動的に従う形にする。
- 経路が増えても受け皿は 1 つ: フレームは噴水符号なので、どの経路から来たフレームも同じ `recvPush` に入れればよい。送信側は 1 つの生成器（SendSession / FrameStream）から各経路に別々のフレームを配る。新しい経路を足すときはこの形を崩さない。
- ローカルネットワーク（preview「Local network boost」、PROTOCOL §12・§13、`web/src/lib/lan.ts`）: 送信側が WebRTC の offer をリンクコードとしてストリームに混ぜ、受信側は確認なしで answer を QR で表示し（ユーザーの判断: LAN は許可を待たなくてよい。音は毎回明示的な許可が必要）、送信側のカメラがそれを読んで接続する（STUN なし、ホスト候補のみ）。RTCPeerConnection は Worker に無いのでページ側で持つ。画面は止めず、`sendTextChannelUp(true)` で末尾から 1 セグメントずつに切り替える。
  - 0.1.3 の方式: 接続上は **バイナリのレコード**（Base45 にしない。受信側が `B1` と名乗らなければ従来のテキスト）で、**噴水符号を使わずソースシンボルを 1 回ずつ**送る（`engine.sendLink` → wasm の `SendSession.nextLink` → core の `DirectSender`）。全部送ったら黙ってフィードバックを待ち、「送った分を全部取り込んだうえで欠けている」と分かったセグメントにだけ修復シンボルを足す（`A<n>` と送信済み数の比較。`applyFeedback(text, linkTaken)`）。受信側はメッセージ（ArrayBuffer）をそのまま Worker に渡し（転送、コピーなし）、`Receive.pushPacked` がまとめて処理する。
  - 送る速さは **ペース**（レコード/秒）を動的に決める（`LanSender`）。「未確認分の上限」だけで渡すとブラウザが一気に送り、SCTP が倍々に増やした末にパケットを落として 1〜3 秒止まる（同じ Mac 内で毎回発生していた）。上げ幅は 1 回 4 MiB/s まで（無制限だと高速域で同じことが起きる）。
  - 実測（同じ Mac、Chrome 2 つ、100 MB、展開込み）: 0.1.2 は 19.9 MiB/s（12.7〜23 でばらつく。必要数の 1.44 倍のコードを送っていた）→ 0.1.3 は 28.5 MiB/s で安定（必要数ちょうど、回線上のバイト数は半分以下、定常時のペースは約 70 MiB/s）。iPhone ⇄ Mac での効果は未確認（ユーザーの計測待ち）。
  - 落とし穴: (1) 受信側が受け取ったメッセージごとに小さな返信を返すとデータチャネルが 15 → 0.6 MB/s に落ちる → 返信は 100 ms ごとに 1 通。(2) 変化が無くても 1 秒に 1 回はフィードバックを送り直す（展開中に黙ると、送信側が 10 秒で「見失った」と判断して全部送り直す）。(3) QR 用の小さなシンボル（60 B）で 1 MiB のセグメントを符号化すると K=17,000 で前計算に数秒かかる → 修復シンボルの前計算は最初に必要になるまで遅らせた（`SegmentEncoder`）。
  - e2e の注意: macOS のファイアウォールは Playwright 同梱の Chromium 同士の LAN アドレス通信を通さない（ループバックを許可すると経路が混ざって遅くなる）。ローカルではインストール済みの Google Chrome（`channel: "chrome"`）を使い、CI（Linux）は同梱 Chromium を使う。`capturePlayer` は 10 fps に追いつかずコマを飛ばすので、全コマが必要なテストは先に Slower を押して表示を遅くする。`QRSEND_LAN_MB=100 QRSEND_LAN_TRACE=1 npx playwright test channels -g "local network"` で大きさを変えて 0.5 秒ごとの経過を見られる。
- CLI 同士の TCP 直結（`qrsend send --lan`、PROTOCOL §12.2、`crates/qrsend/src/net.rs`）: 送信側が待ち受け、アドレス・ポート・鍵を載せたリンクコード（kind 3）をストリームに 6 個に 1 個混ぜる。受信側の CLI はそれを読んだら確認なしで接続する（`--no-lan` でしない）。answer が要らないので送信側にカメラは不要。チャネル上のやり取りは WebRTC のデータチャネルと同じ（バイナリのレコード、`A<n>`、フィードバック）で、全体を ChaCha20-Poly1305 で暗号化する（鍵はコードの鍵＋両側の乱数から導出）。受信側の主ループは「コードからのフレーム」と「接続からのフレーム」を `select!` で同じ `Receiver` に入れる。送信側は表示ループを持つ `FrameStream` がチャネル経由で接続の出来事（Up / Down / Feedback）を受け取り、完了のフィードバックで表示を終える。
  - 実測（同じ Mac、ループバック、非圧縮 200 MB）: 受信完了まで 1.7 秒（約 114 MiB/s）。その後の検証・書き出しに 1.5〜4 秒（既存の処理）。
  - **macOS のファイアウォールは、署名されていない `qrsend` への LAN アドレス宛ての接続を、TCP としては受け付けたあとで切る**（受け入れ側では setsockopt が EINVAL、接続側では EOF）。手元の確認とテストは `--lan-address 127.0.0.1` で行う。実機では送信側で「受け入れますか」を許可する必要がある。2 台の実機間では未検証。
  - CLI ⇄ ブラウザの直結は無い（ブラウザは TCP を使えず、CLI に WebRTC が無い）。CLI の Two-way（カメラでフィードバックを読む）と音も未実装。
- 表示の自動調節（preview「Automatic speed」、PROTOCOL §11.4、core の `tune.rs`）: Two-way のフィードバックの `frames`（読めたコードの数）の増え方で、枚数/秒とコード数/枚（画面に収まる配置の一覧から。正方形とは限らない）を 1 つずつ変えて試し、良くなれば採用・ならなければ戻す。エンジン側（`SendSession.setTuner`、`applyFeedback` の結果の `fps` / `level`）で動かし、ページは `engine.sendTune(配置ごとのコード数, fps, level)` で一覧を渡す。ネットワーク接続中は調節しない。e2e（`auto.spec.ts`）は bridge.ts で 2 ページを生でつなぎ、1 個・10 枚/秒 → 20 秒で 4×2・22 枚/秒（4.9 → 約 54 KiB/s）になるのを確認している。QR の大きさ（バージョン）は変えない（変えるとシンボルサイズが変わり、集めかけのセグメントが無駄になる）。
  - 最初の一手だけは山登りではなく **受信側のカメラの申告** で決める（0.1.3 のリリース後に追加）: 受信側はスキャナーの「1 秒あたりの読み取り回数」と「1 ドットが何ピクセルに写っているか」（`guide.ts` の `dotSize`）を `recvPush(texts, packed, camera)` → `Receive.setCamera` でフィードバックの末尾（`reads` / `dot`、PROTOCOL §11.2）に載せ、送信側の `Tuner::hint` が「読める回数ぶんの枚数」「ドットが 4 px 以上に写る最も密な配置」へ一気に跳ぶ。ページは配置ごとの画面上のドットの大きさ（`dots`）を `sendTune` に渡す。e2e では最初のフィードバックで 1×1・10 枚/秒 → 4×2・27 枚/秒になった。
- カメラの案内（preview「Camera guidance」、`guide.ts`）: デコーダが返すコードの位置（BarcodeDetector の cornerPoints / ZXing の position）と文字数から「1 ドットがカメラの何ピクセルか」を見積もり、中央 192px 四方のラプラシアンで鮮明さを測る。条件が直近の大半で成り立ったら表示、ほぼ消えたら消す（ちらつき防止。ユーザーは以前「Color の表示がついたり消えたり」を指摘している）。
- 音でのフィードバック（preview「Feedback by sound」、PROTOCOL §14、`web/src/lib/sound.ts`）: 送信側は notice の `HEARS_SOUND` を立ててマイクを開く（AudioWorklet の `tap.worklet.js` → メインスレッドの wasm `SoundDecoder`）。受信側は利用者が「Answer by sound」を押したら、フィードバックコードのバイト列を WAV にして `<audio>` で鳴らす（同じコードなら同じ WAV を再生し直す）。CSP の `connect-src 'self'` のためページ内から blob: を fetch できないので、e2e は受信側のコンテキストを `bypassCSP` で作って WAV を読み出し、Chrome の `--use-file-for-fake-audio-capture` で送信側のマイクに流す。実際のスピーカーとマイクでは未検証。
- 版の食い違い（`web/src/lib/update.ts`）: ビルドごとにファイル名（`qrsend_wasm_bg-<hash>.wasm`、Worker の js）が変わり、GitHub Pages は旧版のファイルを残さない。新版がデプロイされた後も開いたままの（または Service Worker が新版に入れ替わった後の）旧ページが、後から自分の版の wasm や Worker を取りに行くと 404 になる（実機で「failed to fetch Wasm: 404」「エンジンの起動に失敗」として発生）。対策: (1) wasm の読み込みは一時的な失敗なら 2 回まで再試行（404 は再試行しない）、失敗を覚えず次回また試す。(2) 読み込み失敗時にサーバーの index.html を取り直し、エントリスクリプト名が違えば新版が出ているので自動で再読み込み（60 秒に 1 回まで）。(3) Service Worker が入れ替わったら、Send / Receive 以外では即再読み込み、Send / Receive では「A new version of QRSend is ready. [Reload]」を出す。
- 速度の目安（実測、2026-10-07。0.1.3 での変化は上の各項目）: CLI のテキスト経路をローカルのパイプで 200 MB → 既定（zstd 19）27 MB/s、`--no-compress` 115 MB/s（エンジン自体はネットワークより速い。既定の圧縮レベルが先に頭打ちになる）。Web の LAN 経路は同じ Mac 内で 25〜26 MiB/s、iPhone ⇄ Mac の Wi-Fi で 5〜6 MB/s（同じ環境で LocalSend は 32 MB/s 近く）。ブラウザは生の TCP を使えず WebRTC データチャネル（SCTP/DTLS、ユーザー空間）になること、フレームを Base45 のテキストで流していること（1.5 倍）、噴水符号・検証・OPFS 書き込みを wasm と JS で行うことが差の候補。どれが効いているかは未特定。
- 計測値の表示（preview「Show measurements」）: カメラの読み取り（毎秒の回数、コードがある画像／無い画像それぞれの所要 ms）、受信側の取り込み（1 回あたりの ms と件数、待ち行列）、LAN 送信側が何を待っているか（受信側の確認待ち／ネットワーク待ち／生成）の割合を画面に出す。「なぜ遅いか」は推測で直さず、まずこれで実機の数字を見る（ユーザーの指摘: 原因を特定してから実装する）。`localStorage["qrsend.debug.zxing"]="1"` で内蔵検出器を使わず ZXing にできる（iPhone 相当の経路を Mac で測る用）。
  - 実測（この Mac、1920×1080）: 内蔵検出器はコードあり 37〜48 ms／なし 25 ms、ZXing はあり 21〜25 ms／なし 28 ms。「コードが映っていないと読み取りが重くなる」は成り立たなかった。実機で「QR を読めなくすると LAN が 5〜6 → 3〜4 MB/s に落ちた」原因は未特定。
- **繰り返し流すものの間隔を固定しない**（`schedule::Recurring`、PROTOCOL §4.1）: Meta の差し込み（10 個に 1 個）と notice / リンクコード（6 個に 1 個など）を固定間隔で流していたため、1 枚に複数のコードを載せると「1 枚の中の同じ場所」にしか現れなかった。0.1.3 の実機テスト（Mac → iPhone、Color codes ＋ Local network boost）で発覚: iPhone は 3 色のうち 1 色しか読めず、217 個読んでも「Waiting for the file list…」のまま、LAN の申し出も届かなかった（Meta は常に 3 層目、notice / offer は常に 1 層目）。間隔を毎回 ±1 揺らして解決。新しく「ときどき流すもの」を足すときは必ず `Recurring` を使う。e2e（`auto.spec.ts` の "a camera that makes out only …"）は bridge で 1 色だけを通して確かめる。
- Meta（ファイル一覧）は大きな転送では長い: セグメントごとの BLAKE3 を持つので 1 GiB で 33,593 B（実測）、1 個 1.2 kB のコードなら K=28。10 個に 1 個の差し込みだと、弱いカメラ（表示の 2 割しか読めない iPhone）ではファイル名が出るまで約 1 分かかった。`Scheduler::meta_every` が「最初の 1 パス分」と「フィードバックで Meta 未受信と分かっている間」は 3 個に 1 個にする。なお LAN 接続は Meta を待たない（offer はセッション ID だけで受け付け、接続後は Meta が最初に届く）。
- 接続までの各段階の時刻（preview「Show measurements」）: 受信側は「最初のコードから、ファイル一覧／offer の読み取り／answer の表示／接続まで何秒か」と「読んだコードのうち notice と offer が何個か」（`rx-steps`）、送信側は「offer を出した／answer を読んだ／接続した」時刻（`tx-steps`）を出す。きれいな映像（e2e、1 色だけ通す）では offer の読み取りまで 0.4〜1.6 秒、そこから answer の表示まで 0.1〜0.2 秒。**実機（Mac → iPhone、0.1.3 ＋修正後）では「ファイル名が出てから LAN の QR が出るまで相当な時間」がかかったが、原因は未特定**。「短いコード（notice / offer）は詰め物が多いので読みにくいのでは」という仮説は、模擬カメラ（ぼかし・ノイズ・2.3〜3.5 px/ドット）での比較で否定された（満杯のコードと同じ率で読める）。次の実機テストでは `rx-steps` の数字をもらう。
- カラーコード（preview、PROTOCOL §2.3）: `drawColorGrid` が 1 枠に 3 コードを RGB で重ねる。受信は `scan.worker.ts` が 12 フレームに 1 回 RGB を分けて読み、3 成分の内容が違えばカラーとして読み続ける（送信側からの合図は無い）。
- 画面キャプチャ受信（preview）: `Scanner.startScreen`（getDisplayMedia）。ヘッドレスのブラウザには共有できる画面が無いので、e2e は getDisplayMedia を仮想カメラのストリームに差し替えて確認している（実際の画面共有は未検証）。
- **e2e でブラウザを起動するときは必ず `--use-fake-device-for-media-stream` を付ける**（`e2e/video.ts` の `fakeCamera`）。`--use-fake-ui-for-media-stream` だけだと権限が自動で通り、受信ページが開発機の本物のカメラを開いてしまう。
- Two-way transfer（逆方向チャネル、0.1.2〜、Web のみ、preview）: 送信側がオンだと SendSession がデータの合間に notice（QSC1）を混ぜる。受信側はそれを見たセッションに限りフィードバック QR を表示し（0.3 秒以上の間隔で描き直す）、送信プレーヤーが自分のカメラ（前面優先）で読んで `engine.sendFeedback` に渡す。途切れたら 2 秒で `sendReceiverSilent(false)`（窓の完成待ちをやめる）、10 秒で `sendReceiverSilent(true)`（全送信に戻る）。`COMPLETE` は展開まで終わってから立てる。フィードバックは認証なしの助言で、到達の証明には使わない。順方向は逆方向に依存させない（PROTOCOL §11.3）。e2e（`two-way.spec.ts`）は 2 つのブラウザの仮想カメラを Y4M でつないで往復させる。

## リリース

- パッケージ: crates.io の `qrsend`（CLI）/ `qrsend-core`、npm の `qrsend-cli`（launcher。コマンド名は `qrsend`。@ なしの `qrsend` は send / resend に似ているとして npm に拒否された）+ 機種別パッケージ（`npm/assemble.py` の TARGETS）。機種別は最初の 8 種だけ `qrsend-bin-<platform>`、0.1.1 以降に追加するものはすべて `@qrsend/cli-bin-<platform>`（npm の組織 `qrsend` を使う。@ なしの新しい名前は spam 判定・類似名判定に引っかかりやすい）。launcher は musl と ARMv6/ARMv7 を実行時に選ぶ。`qrsend-wasm` は publish しない。
- 公開状況（2026-10-08）: **0.1.3 を公開済み**（GitHub Release は 30 個の成果物、crates.io の 2 つ、npm の `qrsend-cli` と機種別 24 個）。手順は 0.1.2 と同じで、publish-all を 1 回（GitHub Releases → npm）＋ crates.io だけをもう 1 回（2 回目は `ref` に `v0.1.3` を指定して、タグの内容を公開する。main が先に進んでいても食い違わない）。npm は Trusted Publishing（OIDC）で全部通る。`qrsend-bin-win32-x64` / `-arm64` は 0.1.0〜0.1.3 とも npm サポートの spam 判定解除待ちで、これがある限り npm の段は「その 2 つだけ失敗」で赤になり、crates.io は単独で再実行が要る（解除後に `scripts/npm-publish-remaining.sh <各バージョン> --trust`）。32bit Windows は `@qrsend/cli-bin-win32-ia32` で確定。npm のレジストリに新しい版が見えるまで 5 分ほどかかる。main はリリース後も 0.1.3 のまま進めている（カメラの申告による初期設定など）ので、次のリリース前に 0.1.4 に上げる。2026-10-07 の 16:50 UTC ごろ、GitHub が `workflow_dispatch` と push に 500 を返す時間帯があった（ステータスページは正常表示）。待って再試行するしかない。
- 手順は igarinpiano/dirlens と同じ方式。`.github/workflows/publish-all.yml` を手動実行（Actions → publish-all → Run workflow）すると GitHub Releases → npm → crates.io の順に公開する。認証は両レジストリとも Trusted Publishing（OIDC）で、トークンはリポジトリに置かない。
  - ビルドは `reusable-build-matrix.yml`（macOS/Windows はネイティブ、それ以外は cross）。必須 8 種に加えて `optional: true` の対象（32bit Windows・Windows GNU・macOS universal・Linux ia32/ARMv7/ARMv6/RISC-V/ppc64le/s390x/LoongArch・Android・FreeBSD/NetBSD/illumos）があり、optional は失敗してもリリースを止めない。`cross: true` はタグ付きリリース版の cross（古い glibc でリンクされる。必須の glibc 版は下限 2.28 を検査）、`cross: git` は main ブランチの cross（LoongArch・NetBSD・Android だけ。イメージが新しく glibc 2.39 になるので他には使わない）。musl・Android・BSD などは `--no-default-features`（window 無し）。
  - `build-check.yml`（手動）は公開せずに全対象をビルドする。対象や依存を変えたらこれで確認する。
  - 機種を追加した直後のリリースでは、新しい npm パッケージに Trusted Publisher が無いので npm の段がそのパッケージ名を挙げて失敗する（他は公開される）。`scripts/npm-publish-remaining.sh <version> --trust` で一度手動公開する（`--trust` は `npm trust` で Trusted Publisher も登録する。npm アカウントは 2FA が auth-and-writes なので、このスクリプトは OTP を入力できる本人の端末で実行する）。npm の段が失敗すると crates.io の段は飛ばされるので、crates.io だけを選んでもう一度実行する。
  - npm の初回公開では `*-win32-*` という名前が spam 判定（403 Forbidden - Package name triggered spam detection）で拒否されやすい（dirlens でも発生）。npm サポートに whitelist を依頼して解除してもらう。途中で止まった公開は `scripts/npm-publish-remaining.sh <version> [--skip NAME]` で再開できる（公開済みは飛ばし、失敗しても続行し、全機種がそろうまで本体 `qrsend-cli` は保留。`--main` で先に公開できる。`--try-name PKG=NAME` はパッケージ PKG の複製を NAME という名前で公開してみて、その名前が npm に通るかだけを調べる）。
  - **初回だけ手動**: publish-all を「GitHub Releases」のみで実行 → `scripts/first-publish.sh <version>`（タグの内容から crates と npm を公開。`--dry-run` あり）→ crates.io / npmjs.com で Trusted Publishing を登録（publish-all.yml、environment は crates が `crates-io`、npm が `npm`）。
- バージョンはワークスペースの `Cargo.toml`（`[workspace.package]`）と `crates/qrsend/Cargo.toml` の qrsend-core 依存、`web/package.json` を揃えて上げる。
- GitHub Pages: リポジトリは public、Pages の Source は「GitHub Actions」。`pages.yml` が main への push ごとに web/ をビルドしてデプロイする（https://igarinpiano.github.io/qrsend/）。Source を「Deploy from a branch」にすると README が表示されてしまうので注意。
- Release には各 OS の CLI、Web アプリのオフライン用 zip（`qrsend-web-<version>.zip`）、SHA256SUMS、build provenance attestation が付く。
