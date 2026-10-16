# Web ページ向けライブラリ（`packages/qrsend-web`）

QRSend の送受信を、ほかの人の Web ページに組み込めるようにした npm パッケージ。利用者向けの説明（英語）は [packages/qrsend-web/README.md](../packages/qrsend-web/README.md)。この文書は、決めたことと中身の記録。

## 決めたこと（2026-10-16、ユーザーと）

- **提供する範囲**: 送りたいデータを QR コードにして表示し、カメラで読み取ってデータに戻すところまで。**暗号化・デバイス ID・信頼済みデバイスは入れない**（平文の転送だけ）。
- **運ぶもの**: テキストだけでなく、ファイル名つきの複数ファイルとフォルダも扱う。
- **出来合いの部品も出す**: 符号化・復号だけでなく、表示（`QrPlayer`）とカメラでの読み取り（`QrScanner`）。
- **パッケージ名は公開するときに決める**。いまは仮に `@qrsend/web` で、`package.json` は `"private": true`（うっかり公開しないため）。
- **商標**: 利用者が自分のページに QR Code の商標表示を出せるよう、README・ソースのコメント・定数 `QR_CODE_TRADEMARK` で案内する。実際に画面に出す文言は利用者が書く。

## 構成

```
crates/qrsend-core/   feature で暗号を外せるようにした（下）
crates/qrsend-js/     ライブラリ用の WebAssembly（wasm-bindgen。publish しない）。Encoder / Decoder
packages/qrsend-web/  npm パッケージ
  src/index.ts        init・Encoder・Decoder・QR_CODE_TRADEMARK（Node でも動く）
  src/player.ts       QrPlayer（canvas に描く）・drawCodes
  src/scanner.ts      QrScanner（カメラ / MediaStream を読む）・readCodes
  src/detect.ts       BarcodeDetector → ZXing（zxing-wasm）の選択
  src/scan-worker.ts  検出を Worker で行う
  scripts/build.mjs   wasm のビルド → wasm-bindgen → tsc → dist/
  test/               Node のテスト（往復、CLI との相互運用）
  example/            見本のページ（送信・受信・自分自身で往復）。Vite でビルドして e2e が使う
  e2e/                Playwright
```

### core の feature

- `crypto`（既定でオン）: `crypto`・`pq` モジュール（age・ed25519・x25519・ML-KEM・ML-DSA・sha3・getrandom・base64・bech32）。`link` を含む。
- `link`: `link`・`linkcert` モジュール（p256・sha2）。
- どちらも無ければ平文の転送だけ。Meta エンベロープは署名つき（方式 2 を含む）でも読める（ML-DSA の署名の長さの定数を `manifest::PQ_SIGNATURE` に移した。`pq::SIGNATURE` はそれを指す）。署名は確かめない。
- CLI・fuzz は既定のまま、Web アプリ用の `qrsend-wasm` は `features = ["ruzstd", "crypto"]` を明示した。
- 大きさ: ライブラリの wasm は 875 kB（gzip で 320 kB）。アプリ用は 2.8 MB（wasm-bindgen 前）。

### 送信（`Encoder`）

- 中身はすべて wasm のメモリに置く（`MemorySource`）。QR で運べる量（数十 kB/s）には十分で、数百 MB はアプリか CLI を使ってもらう（README に書いた）。
- セッション ID は内容から決める（`content_session_id`。アプリの平文と同じ）。同じデータを送り直すと、途中で止まった受信側が続きから受け取れる。
- QR の大きさは既定で `qr::auto_params`（小さな転送は小さなコード、ほかはバージョン 25-L）。指定するときの誤り訂正の既定は M。
- **受信側が断るパス（`..`、絶対パス）は送信側で断る**（`SafePath::parse`）。最初は受け付けていて、受信側の展開が「unsafe path」で失敗した（テストで発覚）。大文字小文字だけ違う名前や Windows で使えない名前は、従来どおり受信側で付け替える。
- 圧縮は ruzstd（アプリと同じ。レベル 1 相当）。

### 受信（`Decoder`）

- 最初に読んだ転送に固定し、ほかの転送のコードは読み飛ばす（`otherTransfer`）。QRSend のコードでない文字列、notice（QSC1）やリンクコード（QSL1）も読み飛ばす。
- **暗号化された転送は、固定する前に読み飛ばす**（`encrypted` を立てる）。暗号化された転送に固定してしまうと、あとから平文の転送が映っても受け取れなくなるため。
- 検証はアプリと同じ: セグメントの BLAKE3（manifest 到着前の分は到着時に）、全体の BLAKE3、ファイルごとの BLAKE3。パスは `payload::unpack` が無害化する。
- `push` は「progress が変わったか」を返す（毎回 `progress` を JSON にしないため）。

### 表示（`QrPlayer`）

- requestAnimationFrame で、`fps` ごとに 1 枚描く。`grid` は 1（既定）・数・`[列, 行]`・`"fill"`。余白は 4 モジュールを隣と共有（`qr::QUIET` と同じ）。1 モジュールは整数ピクセル（`devicePixelRatio` 込み）。
- アプリのカラー表示・自動調節・Two-way・LAN は入れていない。

### 読み取り（`QrScanner`）

- 検出は BarcodeDetector（QR に対応しているとき）→ ZXing。ZXing の wasm は、`zxingWasmUrl` を渡さなければ zxing-wasm の既定（jsDelivr の CDN）から取る。BarcodeDetector がある端末では取りに行かない。
- 既定で Worker（`new URL("./scan-worker.js", import.meta.url)`）。Worker が作れない・読み込めないときは、ページ上で検出する（`worker: false` と同じ）。
- 1 枚読み終わるまで次の 1 枚を取らない（requestVideoFrameCallback、無ければ requestAnimationFrame）。
- `receive()` は完了したらカメラを閉じて結果を返す。`stop()` で AbortError、もう一度 `receive()` すると続きから（`reset()` で最初から）。

## 確かめたこと

- Rust の単体テスト（`crates/qrsend-js`。3 分の 1 を落として逆順でも組み上がる、同じデータは同じ転送、指定したバージョンの大きさ）。`cargo test -p qrsend-core --no-default-features --features ruzstd`（暗号なしの core）。
- Node（`npm test`）: テキスト・ファイル・フォルダ・Blob・バイト列の往復、コードの欠けと順不同、パスの扱い、同じデータは同じ転送、別の転送・無関係な文字列の読み飛ばし、進み具合が減らないこと。**CLI との相互運用**: CLI の `send --plain --export-text` をライブラリで読む、ライブラリの出力を CLI の `recv --text` で読む、CLI の暗号化した転送は `encrypted` として読み飛ばす。
- ブラウザ（`npm run test:e2e`、Chromium）: 見本のページを Vite でビルドし、プレーヤーの canvas を `captureStream` でスキャナーに渡して往復（Worker あり・なし、1 枚に 4 個）。Chrome の仮想カメラに CLI の `--export-y4m` を流して、受信の見本ページで受け取る。送信の見本ページに商標表示が出る。
- npm で入れた形: `npm pack` したものを別のディレクトリの Vite のプロジェクトに入れ、設定なしで、開発サーバーでも本番ビルドでも往復できた（wasm と Worker が出力に入る）。
- **未確認**: 実機のカメラ（とくに iPhone の Safari = ZXing）、BarcodeDetector の経路（Linux の Chromium には無い）、webpack など Vite 以外のバンドラ、バンドラなし（import map）、アプリ（Web 版）との相互運用（同じ core なので通るはずだが、テストは CLI とだけ）。

## 公開するときにやること（未実施）

1. 名前を決める: `package.json` の `name`、README、`example/vite.config.js` の alias、ソースのコメント（`@qrsend/web/player` など）を置き換える。npm の組織 `qrsend` の下なら、新しい名前でも spam 判定に引っかかりにくい。
2. `package.json` の `"private": true` を外す。
3. `publish-all.yml` に npm の段を足す（`packages/qrsend-web` で `npm run build` → `npm publish --provenance`）。機種別のパッケージは無いので 1 つだけ。
4. 新しいパッケージなので、最初の 1 回は手で公開し、Trusted Publisher を登録する（CLI の機種別パッケージと同じ手順）。
5. バージョンはワークスペースの版と揃える（`packages/qrsend-web/package.json` を、版上げの対象に足した）。

---

QR コードは株式会社デンソーウェーブの登録商標です。QR Code is a registered trademark of DENSO WAVE INCORPORATED in Japan and in other countries.
