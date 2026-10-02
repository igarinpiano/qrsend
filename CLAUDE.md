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
  input               画像・Y4M・ffmpeg 経由の動画
```

## コマンド

```bash
cargo test --workspace                 # 全テスト（core の単体＋CLI の E2E）
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo build --release -p qrsend --no-default-features   # ウィンドウ無しビルド
```

手動 E2E（実カメラ無しで往復できる）:

```bash
export QRSEND_DATA_DIR=/tmp/q/data QRSEND_CACHE_DIR=/tmp/q/cache   # 本物の inbox/cache を汚さない
qrsend send some/dir --export-frames /tmp/q/f   # or --export-y4m v.y4m
qrsend recv --images /tmp/q/f -o /tmp/q/out
```

## 実装上の注意

- `cargo fmt` で整形されるため、スクリプトで文字列置換して編集すると一致しないことがある。編集は Read → Edit で行う。
- core は WASM でも使うので、スレッド・ファイルシステム・プロセスを直接使わない。zstd は現状ネイティブ（C）。WASM ビルド時に要対応。
- QR は `qr::render` が常に単一の英数字セグメントで符号化する（汎用の最適化器だと固定バージョンで容量オーバーする実例があった）。容量は `QrParams::symbol_size()` で決まる。
- 受信側は frame の CRC → セッション整合 → RaptorQ → セグメント BLAKE3（manifest 到着後）→ 全体 BLAKE3 → ファイル BLAKE3 の順に検証する。manifest 到着前のセグメントは `unverified` として保存し、後で検証する。
- `recv` は未完了で終わると終了コード 2 と resume code を出す。

## リリース

- パッケージ名: crates.io の `qrsend`（CLI）/ `qrsend-core`、npm の `qrsend` はいずれも未取得だった（2026-10 時点）。まだ publish していない。
- `v*` タグの push で `.github/workflows/release.yml` が各 OS のバイナリを作り GitHub Release に添付する。crates.io への publish は `CARGO_REGISTRY_TOKEN` シークレットがある場合のみ実行。
- バージョンはワークスペースの `Cargo.toml`（`[workspace.package]`）で一括管理。
