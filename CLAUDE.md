# QRSend — プロジェクトメモ（Claude 向け）

QR コードのストリームでテキスト・ファイル・フォルダを送るツール。CLI（Rust 単一バイナリ）とブラウザ版（GitHub Pages / PWA、開発中）の 2 本立て。

- 構想・決定事項: `docs/CONCEPT.md`
- ワイヤ形式の仕様: `docs/PROTOCOL.md`（**プロトコルに触れる変更は必ずここも更新する**）

## ユーザーについて

- やり取りは日本語。docs も日本語。**アプリの UI・CLI のメッセージ・README は英語のみ**。
- Rust 好き。Web フロントエンドには詳しくないので、Web 側の判断は具体例つきで説明する。
- 方針として決まっていること: 大容量優先（時間がかかるのは可、圧縮は手を抜かない）、暗号化は公開鍵方式（age ＋ 送信者署名、Trusted devices）、UI フレームワークは Svelte 5。

## 構成

```
crates/qrsend-core/  プロトコル本体（I/O は std::io トレイトのみ。WASM と共有する）
  base45 / frame      フレーム形式（ヘッダ 22B + シンボル + CRC32、Base45 で QR 英数字モード）
  fec                 RaptorQ（raptorq crate）。1 セグメント = 1 ソースブロック、ESI は RFC 6330 準拠
  schedule / sender   参考送信スケジュール（窓インターリーブ、Meta は毎パス＋差し込み）
  receiver            受信状態機械（暗号・I/O なし。Event を返す）
  manifest            manifest JSON とメタエンベロープ（"QSM"、署名枠つき）
  payload             ファイル内容の単純連結＋zstd、BLAKE3 検証、展開（UnpackSink）
  sanitize / resume   パス無害化、resume code（QSR1-…）
  qr                  QR 生成（単一英数字セグメント固定）と検出（rqrr）
crates/qrsend/       CLI（バイナリ名 qrsend）
  spool               送信データをキャッシュに固めたもの（resume 用に 14 日保持）
  store / recv        受信セッション（inbox）。暗号文のまま保存→完了後に展開
  display/            window（minifb, feature "window"）/ terminal（crossterm）/ export（PNG, Y4M）
  input / decode      画像・Y4M・ffmpeg 経由の動画 → rxing（ZXing 移植）で検出、ダメなら rqrr
  identity / devices  デバイス ID（X25519+Ed25519）と Trusted devices（QRSEND_CONFIG_DIR）
crates/qrsend-wasm/  Web 用バインディング（wasm-bindgen）。core を ruzstd で使う。全部メモリ上
web/                 Svelte 5 + Vite + TS の PWA
  src/lib/            core.ts（wasm 読み込み）/ db.ts（IndexedDB）/ scanner.ts + scan.worker.ts
                      （BarcodeDetector → zxing-wasm）/ inbox.ts（受信の永続化）/ save.ts
  src/views/          Home / Send(+Player) / Receive(+Camera, Result) / Devices / Inbox
  e2e/                Playwright。CLI が書いた Y4M を Chrome の仮想カメラに流す相互運用テスト
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
cargo build --release -p qrsend && npx playwright test   # e2e（CLI バイナリを使う）
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
- CLI のデコードは rxing。rqrr はスクリーンショットや縮小動画に弱かった（実測で 0/6 vs 6/6）。
- Web のキーは今のところ IndexedDB に秘密鍵文字列で保存（CONCEPT にある non-extractable WebCrypto 化は未実装）。
- Web 版は全データをメモリに載せる（大容量は CLI 推奨。OPFS ストリーミングは未実装）。
- QR は `qr::render` が常に単一の英数字セグメントで符号化する（汎用の最適化器だと固定バージョンで容量オーバーする実例があった）。容量は `QrParams::symbol_size()` で決まる。
- 受信側は frame の CRC → セッション整合 → RaptorQ → セグメント BLAKE3（manifest 到着後）→ 全体 BLAKE3 → ファイル BLAKE3 の順に検証する。manifest 到着前のセグメントは `unverified` として保存し、後で検証する。
- `recv` は未完了で終わると終了コード 2 と resume code を出す。

## リリース

- パッケージ名: crates.io の `qrsend`（CLI）/ `qrsend-core`、npm の `qrsend` はいずれも未取得だった（2026-10 時点）。まだ publish していない。
- `v*` タグの push で `.github/workflows/release.yml` が各 OS のバイナリを作り GitHub Release に添付する。crates.io への publish は `CARGO_REGISTRY_TOKEN` シークレットがある場合のみ実行。
- バージョンはワークスペースの `Cargo.toml`（`[workspace.package]`）で一括管理。
- GitHub Pages: `.github/workflows/pages.yml` は毎回ビルドし、リポジトリが public のときだけデプロイする。2026-10 時点でリポジトリは private で、現在のプランでは Pages を有効化できない（API が 422）。公開する場合は Settings → Pages → Source を「GitHub Actions」にする。
- Release には各 OS の CLI に加え、Web アプリのオフライン用 zip（`qrsend-web-<tag>.zip`）が付く。
