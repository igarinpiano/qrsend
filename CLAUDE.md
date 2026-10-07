# QRSend — プロジェクトメモ（Claude 向け）

QR コードのストリームでテキスト・ファイル・フォルダを送るツール。CLI（Rust 単一バイナリ）とブラウザ版（GitHub Pages / PWA）の 2 本立て。

- 構想・決定事項: `docs/CONCEPT.md`
- ワイヤ形式の仕様: `docs/PROTOCOL.md`（**プロトコルに触れる変更は必ずここも更新する**）

## ユーザーについて

- やり取りは日本語。docs も日本語。**アプリの UI・CLI のメッセージ・README は英語のみ**。
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
  link                別の経路を張るためのリンクコード（QSL1-…、分割と組み立ての枠だけ。中身は経路ごと）。PROTOCOL §12
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
crates/qrsend-wasm/  Web 用バインディング（wasm-bindgen）。コールバック方式: SendJob（本体を JS の write に書き出す）/ SendSession（JS の read からセグメントを読む）/ Receive（完成セグメントを JS に渡す。extract は read → sink へストリーミング）
web/                 Svelte 5 + Vite + TS の PWA
  src/lib/engine.worker.ts  エンジン本体（Worker）。wasm とストレージを持ち、送信の梱包・フレーム生成・受信状態・展開を担当。ページとは engine.ts の RPC でやり取り
  src/lib/storage.ts        OPFS の同期アクセスハンドル（無ければメモリ）。受信は recv/<session>/{body,meta,out}、送信は send/<uuid>/body
  src/lib/keys.ts           デバイス ID。WebCrypto の non-extractable 鍵（X25519 / Ed25519）。非対応ブラウザは legacy（wasm 内の鍵）。旧形式（文字列）は初回に自動移行
  src/lib/                  core.ts（wasm 読み込み）/ db.ts（IndexedDB: 鍵・信頼デバイス・セッション一覧）/ scanner.ts + scan.worker.ts（BarcodeDetector → zxing-wasm、カメラと動画ファイル）/ save.ts + zip.ts（ディスクから直接保存、ZIP64 対応の無圧縮 ZIP）/ qrdraw.ts
  src/views/                Home / Send(+Player) / Receive(+Camera, Result) / Devices / Inbox / Preview（Feature preview）
  e2e/                      Playwright。CLI が書いた Y4M を Chrome の仮想カメラに流す相互運用テスト（暗号化・多セグメント・ZIP を含む）
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
- Web の暗号化セッション: age ヘッダの X25519 スタンザごとに WebCrypto で共有秘密を計算して wasm に渡す（Meta と Body は別の age ファイルなので 2 回）。署名は SendJob.seal() が返すメッセージを WebCrypto で署名して finish() に渡す。
- OPFS はオリジン共有なので、送信スプールは Worker ごとの UUID ディレクトリに置く。
- QR は `qr::render` が常に単一の英数字セグメントで符号化する（汎用の最適化器だと固定バージョンで容量オーバーする実例があった）。容量は `QrParams::symbol_size()` で決まる。
- 受信側は frame の CRC → セッション整合 → RaptorQ → セグメント BLAKE3（manifest 到着後）→ 全体 BLAKE3 → ファイル BLAKE3 の順に検証する。manifest 到着前のセグメントは `unverified` として保存し、後で検証する。
- `recv` は未完了で終わると終了コード 2 と resume code を出す。
- **作りかけの機能は Feature preview に置く**（ユーザーの方針: 基本は従来の方式。新機能は送信側が `#/preview` で個別にオンにする）。定義は `web/src/lib/prefs.ts` の `PREVIEW_FEATURES`、保存先は localStorage の `qrsend.preview.<id>`。オフのときは画面も送る内容も従来と同じにする。受信側には設定を作らず、送信側からの合図で自動的に従う形にする。
- 経路が増えても受け皿は 1 つ: フレームは噴水符号なので、どの経路から来たフレームも同じ `recvPush` に入れればよい。送信側は 1 つの生成器（SendSession / FrameStream）から各経路に別々のフレームを配る。新しい経路を足すときはこの形を崩さない。
- ローカルネットワーク（preview「Local network boost」、PROTOCOL §12・§13、`web/src/lib/lan.ts`）: 送信側が WebRTC の offer をリンクコードとしてストリームに混ぜ、受信側は利用者が Connect を押したら answer を QR で表示、送信側のカメラがそれを読んで接続する（STUN なし、ホスト候補のみ）。RTCPeerConnection は Worker に無いのでページ側で持ち、コードは `engine.sendTexts`（4096 バイトのシンボルの別フレーム列。SendSession の `wide`）で取り出して流す。画面は止めず、`sendTextChannelUp(true)` で末尾から 1 セグメントずつに切り替える。
  - 実測で分かった落とし穴: (1) 受信側が受け取ったメッセージごとに小さな返信を返すとデータチャネルが 15 → 0.6 MB/s に落ちる → 返信は 100 ms ごとに 1 通にまとめる。(2) 接続直後に一気に流し込むとパケットが捨てられて 8 秒止まる → 送ってよい量は 32 コードから倍々に増やす動的な窓（`LanSender`）。(3) QR 用の小さなシンボル（60 B）で 1 MiB のセグメントを符号化すると K=17,000 で前計算に数秒かかる → 修復シンボルの前計算は最初に必要になるまで遅らせた（`SegmentEncoder`）。
  - e2e の注意: macOS のファイアウォールは Playwright 同梱の Chromium 同士の LAN アドレス通信を通さない（ループバックを許可すると経路が混ざって遅くなる）。ローカルではインストール済みの Google Chrome（`channel: "chrome"`）を使い、CI（Linux）は同梱 Chromium を使う。`capturePlayer` は 10 fps に追いつかずコマを飛ばすので、全コマが必要なテストは先に Slower を押して表示を遅くする。
- カラーコード（preview、PROTOCOL §2.3）: `drawColourGrid` が 1 枠に 3 コードを RGB で重ねる。受信は `scan.worker.ts` が 12 フレームに 1 回 RGB を分けて読み、3 成分の内容が違えばカラーとして読み続ける（送信側からの合図は無い）。
- 画面キャプチャ受信（preview）: `Scanner.startScreen`（getDisplayMedia）。ヘッドレスのブラウザには共有できる画面が無いので、e2e は getDisplayMedia を仮想カメラのストリームに差し替えて確認している（実際の画面共有は未検証）。
- **e2e でブラウザを起動するときは必ず `--use-fake-device-for-media-stream` を付ける**（`e2e/video.ts` の `fakeCamera`）。`--use-fake-ui-for-media-stream` だけだと権限が自動で通り、受信ページが開発機の本物のカメラを開いてしまう。
- Two-way transfer（逆方向チャネル、0.1.2〜、Web のみ、preview）: 送信側がオンだと SendSession がデータの合間に notice（QSC1）を混ぜる。受信側はそれを見たセッションに限りフィードバック QR を表示し（0.3 秒以上の間隔で描き直す）、送信プレーヤーが自分のカメラ（前面優先）で読んで `engine.sendFeedback` に渡す。途切れたら 2 秒で `sendReceiverSilent(false)`（窓の完成待ちをやめる）、10 秒で `sendReceiverSilent(true)`（全送信に戻る）。`COMPLETE` は展開まで終わってから立てる。フィードバックは認証なしの助言で、到達の証明には使わない。順方向は逆方向に依存させない（PROTOCOL §11.3）。e2e（`two-way.spec.ts`）は 2 つのブラウザの仮想カメラを Y4M でつないで往復させる。

## リリース

- パッケージ: crates.io の `qrsend`（CLI）/ `qrsend-core`、npm の `qrsend-cli`（launcher。コマンド名は `qrsend`。@ なしの `qrsend` は send / resend に似ているとして npm に拒否された）+ 機種別パッケージ（`npm/assemble.py` の TARGETS）。機種別は最初の 8 種だけ `qrsend-bin-<platform>`、0.1.1 以降に追加するものはすべて `@qrsend/cli-bin-<platform>`（npm の組織 `qrsend` を使う。@ なしの新しい名前は spam 判定・類似名判定に引っかかりやすい）。launcher は musl と ARMv6/ARMv7 を実行時に選ぶ。`qrsend-wasm` は publish しない。
- 公開状況（2026-10-07）: 0.1.1 を公開済み。GitHub Releases（28 機種＋Web zip）と crates.io は publish-all、npm は `qrsend-cli` と機種別 24 個（`qrsend-bin-*` の darwin / linux 6 個、`@qrsend/cli-bin-*` 18 個）。npm のうち `qrsend-bin-darwin-arm64` 以外は Trusted Publisher が効かなかったため `scripts/npm-publish-remaining.sh 0.1.1 … --trust --main` で手動公開し、同時に全パッケージへ Trusted Publisher を登録した（次のリリースからは publish-all だけで出るはず。未検証）。`qrsend-bin-win32-x64` / `-arm64` は 0.1.0・0.1.1 とも npm サポートの spam 判定解除待ち（0.1.1 は今回だけ Windows 抜きでリリース。解除後に `scripts/npm-publish-remaining.sh 0.1.0 --trust` と `… 0.1.1 --trust`）。32bit Windows を `qrsend-bin-win32-ia32` という名前で公開する試みも spam 判定で拒否されたので、`@qrsend/cli-bin-win32-ia32` で確定。
- 手順は igarinpiano/dirlens と同じ方式。`.github/workflows/publish-all.yml` を手動実行（Actions → publish-all → Run workflow）すると GitHub Releases → npm → crates.io の順に公開する。認証は両レジストリとも Trusted Publishing（OIDC）で、トークンはリポジトリに置かない。
  - ビルドは `reusable-build-matrix.yml`（macOS/Windows はネイティブ、それ以外は cross）。必須 8 種に加えて `optional: true` の対象（32bit Windows・Windows GNU・macOS universal・Linux ia32/ARMv7/ARMv6/RISC-V/ppc64le/s390x/LoongArch・Android・FreeBSD/NetBSD/illumos）があり、optional は失敗してもリリースを止めない。`cross: true` はタグ付きリリース版の cross（古い glibc でリンクされる。必須の glibc 版は下限 2.28 を検査）、`cross: git` は main ブランチの cross（LoongArch・NetBSD・Android だけ。イメージが新しく glibc 2.39 になるので他には使わない）。musl・Android・BSD などは `--no-default-features`（window 無し）。
  - `build-check.yml`（手動）は公開せずに全対象をビルドする。対象や依存を変えたらこれで確認する。
  - 機種を追加した直後のリリースでは、新しい npm パッケージに Trusted Publisher が無いので npm の段がそのパッケージ名を挙げて失敗する（他は公開される）。`scripts/npm-publish-remaining.sh <version> --trust` で一度手動公開する（`--trust` は `npm trust` で Trusted Publisher も登録する。npm アカウントは 2FA が auth-and-writes なので、このスクリプトは OTP を入力できる本人の端末で実行する）。npm の段が失敗すると crates.io の段は飛ばされるので、crates.io だけを選んでもう一度実行する。
  - npm の初回公開では `*-win32-*` という名前が spam 判定（403 Forbidden - Package name triggered spam detection）で拒否されやすい（dirlens でも発生）。npm サポートに whitelist を依頼して解除してもらう。途中で止まった公開は `scripts/npm-publish-remaining.sh <version> [--skip NAME]` で再開できる（公開済みは飛ばし、失敗しても続行し、全機種がそろうまで本体 `qrsend-cli` は保留。`--main` で先に公開できる。`--try-name PKG=NAME` はパッケージ PKG の複製を NAME という名前で公開してみて、その名前が npm に通るかだけを調べる）。
  - **初回だけ手動**: publish-all を「GitHub Releases」のみで実行 → `scripts/first-publish.sh <version>`（タグの内容から crates と npm を公開。`--dry-run` あり）→ crates.io / npmjs.com で Trusted Publishing を登録（publish-all.yml、environment は crates が `crates-io`、npm が `npm`）。
- バージョンはワークスペースの `Cargo.toml`（`[workspace.package]`）と `crates/qrsend/Cargo.toml` の qrsend-core 依存、`web/package.json` を揃えて上げる。
- GitHub Pages: リポジトリは public、Pages の Source は「GitHub Actions」。`pages.yml` が main への push ごとに web/ をビルドしてデプロイする（https://igarinpiano.github.io/qrsend/）。Source を「Deploy from a branch」にすると README が表示されてしまうので注意。
- Release には各 OS の CLI、Web アプリのオフライン用 zip（`qrsend-web-<version>.zip`）、SHA256SUMS、build provenance attestation が付く。
