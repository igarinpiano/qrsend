# QRSend 構想

> ステータス: 構想（v0 設計メモ）。プロトコルの詳細は `docs/PROTOCOL.md`（未作成）で確定させる。

## 1. コンセプト

**ネットワークもアカウントも要らない。画面とカメラさえあれば、どんなデータでも送れる。**

- 送信側はデータを **QR コードのアニメーション** として流し続け、受信側はカメラでそれを読み取る。
- 光（画面 → カメラ）による一方向通信なので、Wi-Fi が無い場所・別ネットワーク間・エアギャップ環境でも動く。
- **CLI（単一バイナリ）とブラウザ版（GitHub Pages / PWA）の 2 本立て**。どちらからどちらへも送受信できる。
- 転送に時間がかかることは許容する。その代わり **大容量でも、何時間・何日かかっても確実に届く** ことを最優先する。
- LocalSend のような手軽さで使えて、公開鍵ベースのエンドツーエンド暗号化を標準にする。

## 2. 決定事項

| 項目 | 決定 |
|---|---|
| 提供形態 | CLI（Rust 単一バイナリ）＋ ブラウザ版（静的サイト / PWA） |
| 中核実装 | Rust（`qrsend-core`）。CLI はネイティブビルド、ブラウザは WebAssembly ビルド |
| ブラウザ UI | Svelte 5 + Vite + TypeScript |
| UI 言語 | 英語のみ |
| 優先度 | 大容量 ＞ 速度。時間がかかるのは可、圧縮は手を抜かない |
| 圧縮 | zstd（圧縮が効かないデータは zstd 自体が raw ブロックで格納するため実害なし） |
| 暗号化 | 公開鍵方式（age 形式準拠）＋ 送信者署名。信頼済みデバイス（Trusted devices）で管理 |
| 誤り耐性 | RaptorQ（RFC 6330）ファウンテン符号 ＋ セグメント分割 |
| ホスティング | GitHub Pages（Web）、GitHub Releases（CLI バイナリ・単一 HTML 版） |

## 3. ユースケース（想定は幅広い）

- PC ⇄ スマホ、スマホ ⇄ スマホ、PC ⇄ PC の日常的なファイル・テキスト受け渡し
- ネットワークに繋がっていない（繋げたくない）端末とのデータ移送
- 1 つの送信画面を複数人が同時に読む一斉配布（age の複数受信者機能を利用）
- 送信画面を動画で撮影しておき、後から CLI でまとめて解析する大容量転送

## 4. アーキテクチャ

```
          ┌──────────────── qrsend-core (Rust) ────────────────┐
          │ manifest + 連結ペイロード + zstd / age 暗号 / 署名  │
          │ セグメント分割 / RaptorQ / フレーム形式 / パス無害化   │
          └─────────┬───────────────────────────┬──────────────┘
              native build                 wasm build
                    │                           │
            qrsend-cli (単一バイナリ)     qrsend-wasm ─ web (Svelte UI)
```

```
qrsend/
  crates/
    qrsend-core/    # プロトコル本体。I/O を持たない純粋ロジック（テストの中心）
    qrsend-cli/     # CLI バイナリ（表示・カメラ・ファイル I/O）
    qrsend-wasm/    # wasm-bindgen によるブラウザ向けバインディング
  web/              # Vite + Svelte + TS、PWA、Web Worker
  docs/             # CONCEPT.md / PROTOCOL.md
  .github/workflows # CI / Pages デプロイ / Release
```

`qrsend-core` を CLI とブラウザで共有することで、両者のプロトコル挙動が食い違わないことを構造的に保証する。

## 5. データパイプライン

### 送信

```
files / dirs / text / stdin
  → ファイル内容を manifest 順に単純連結（ストリーミング）→ zstd 圧縮
  → age 暗号化（受信者の公開鍵宛て、複数可）→ Body
  → 署名付き manifest → Meta（セグメント 0）
  → Body をセグメント分割（既定 1 MiB 単位）
  → RaptorQ 符号化（各セグメントごと）
  → フレーム化（ヘッダ + payload + CRC、Base45 / QR 英数字モード）
  → QR 描画ループ（無限に流し続ける）
```

### 受信

```
カメラ / 動画ファイル / 画像群
  → QR デコード → フレーム解析（sessionId で他の送信を排除）
  → セグメントごとに RaptorQ 復号 → 一時領域へ保存（暗号文のまま）
  → 先頭から連続して揃った分を順次:
       age 復号 → zstd 展開 → manifest に従って切り出し → ファイル単位で BLAKE3 検証
  → 保存先へ書き出し
```

- 全工程をストリーミング処理にし、データ全体をメモリに載せない。
- 一時領域には **暗号文のまま** 保存する（途中で放置されても中身が平文で残らない）。
- manifest（ファイル一覧・サイズ・BLAKE3 ハッシュ・送信者公開鍵・署名）は先頭セグメントに入る。先頭セグメントは他より高頻度で流し、受信開始直後に「何が・誰から届くか」を表示できるようにする。

## 6. セキュリティ

### 6.1 デバイス ID と信頼済みデバイス

各デバイスは初回起動時に ID を生成する。

- **X25519 鍵ペア**: 受信用（age の recipient / identity）
- **Ed25519 鍵ペア**: 送信者署名用
- **フィンガープリント**: 公開鍵から導出した短い確認コード（例: `482-913`）

ペアリング（LocalSend のデバイス一覧に相当）:

1. 受信側が自分の ID を QR 1 枚で表示（`qrsend id` / Web の *Devices → Show my ID*）
2. 送信側がそれを読み取る（カメラ、または `age1...` 形式の文字列を貼り付け）
3. 両方の画面に出る確認コードが一致することを目視で確認 → *Trusted devices* に登録
4. 逆方向も行えば相互ペアリングとなり、受信時に「誰から届いたか」を検証できる

### 6.2 脅威と対策

| 脅威 | 対策 |
|---|---|
| 盗み見・盗撮（QR は誰でも読める） | age による公開鍵暗号化。ファイル名などの manifest も暗号化 |
| なりすまし送信（公開鍵は誰でも知り得る） | manifest に Ed25519 署名。信頼済みの送信者なら `From: <device> ✓`、未知なら `Unverified sender` と明示 |
| 改ざん・混入 | age の AEAD（チャンクごとの認証）＋ ファイル単位の BLAKE3 検証 ＋ フレーム CRC ＋ sessionId |
| 悪意あるファイル | パス無害化（絶対パス・`..`・Windows 予約名・不正文字を拒否）、manifest 宣言サイズを超える展開を中止（圧縮爆弾対策）、受信ファイルを自動で開かない |
| 配布元の汚染 | Web は外部通信を一切しない（CSP `connect-src 'none'`）。Release 成果物に GitHub Artifact Attestation を付与し `gh attestation verify` で検証可能に |
| 残骸 | 一時領域は暗号文のみ。保存完了後に削除（設定で保持期間を変更可） |
| 鍵の流出（Web） | X25519 秘密鍵は WebCrypto の **non-extractable** キーとして IndexedDB に保存し、JS からも読み出せないようにする。ECDH の結果のみ WASM に渡す |
| 鍵の流出（CLI） | `~/.config/qrsend/identity`（権限 0600、任意でパスフレーズ保護） |

### 6.3 暗号化ポリシー

- 送信先として信頼済みデバイスを選ぶのが基本。**暗号化が標準**。
- 不特定の相手に送る平文モード（`--plain` / Web の *Anyone (unencrypted)*）は、明示的に選んだ場合のみ使える。
- 複数の受信者を同時に指定できる（一斉配布）。宛先に含まれないデバイスでは「This transfer is not addressed to this device」と表示する。

## 7. 大容量への対応

### 7.1 所要時間の目安（実測前の推定・圧縮前）

| データ量 | 標準 QR（約 10KB/s） | 高密度＋グリッド（約 30KB/s） | 将来の高速モード（約 100KB/s） |
|---|---|---|---|
| 10MB | 約 17 分 | 約 6 分 | 約 2 分 |
| 100MB | 約 2.8 時間 | 約 1 時間 | 約 17 分 |
| 1GB | 約 29 時間 | 約 10 時間 | 約 3 時間 |

### 7.2 仕組み

1. **ストリーミング処理**: 送受信ともファイル全体をメモリに載せない。
2. **セグメント分割**: 数 MB 単位のセグメントごとに RaptorQ で符号化する。完成したセグメントは確定して保存し、メモリを解放する。
3. **送信スケジュール**: 全セグメントを巡回し、各セグメントに少し多めの修復シンボルを付けて流す。1 周で大半が完成するようにし、その後も新しい修復シンボルで巡回を続ける。
4. **欠損セグメントの再送（片方向通信を補う仕組み）**: 受信側が欠けているセグメントを短い *resume code*（QR または文字列）で示し、送信側は `--resume <code>` でその分だけを流す。カメラが無ければ手入力でもよい。
5. **再開**: 受信状態は sessionId 単位で永続化する。中断して何日後でも、同じ送信を再び映せば続きから受信できる。
6. **録画モード**: 送信画面をスマホで動画撮影しておき、`qrsend recv --video rec.mp4` で後からまとめて解析する。ライブでのデコード速度に縛られないので、送信側はより高い fps で流せる。

## 8. 受信から保存までの流れ

CLI と Web は同じモデルで動く: **Receive（集める）→ Inbox（受信箱）→ Save（書き出す）**。
小さな転送では Inbox を意識させず、完了と同時に保存まで自動で進める。大きな転送では Inbox が「中断・再開・後から書き出し」の拠点になる。

### 8.1 CLI

```
$ qrsend recv                      # 既定: カメラ 0、出力先はカレントディレクトリ
  Camera: FaceTime HD Camera
  From:   chimo-macbook ✓ (482-913)          ← 署名検証済みの送信者
  Files:  photos/ (1,204 files), notes.md     ← 復号した manifest
  Size:   2.31 GB (compressed 2.05 GB)
  Disk:   OK (48.2 GB free)
  [████████░░░░░░░░░░░░] 41%  seg 412/1003  27.4 KB/s  ETA 6h12m
```

1. **開始前チェック**: 宛先が自分か、送信者が信頼済みか、ディスクの空き容量は十分かを確認する。
2. **受信中**: 受信状態は状態ディレクトリ（`~/Library/Application Support/qrsend/sessions/<id>/` 等、OS 標準の場所）に暗号文のまま保存する。`Ctrl-C` で中断しても、もう一度 `qrsend recv` すれば自動で再開する。
3. **順次書き出し**: 先頭から連続して揃った部分を、`<out>/.qrsend-<id>.partial/` に復号・展開しながら書き出す。各ファイルは BLAKE3 で検証する。
4. **完了**: 全ファイルの検証に成功したら `<out>/` へ移動し、セッションの一時データを削除する（`--keep` で保持）。同名ファイルがある場合は `--on-conflict rename|overwrite|skip` に従う（既定は `rename`）。
5. **その他の出口**:
   - `--stdout`: 単一ファイルやテキストを標準出力へ（例: `qrsend recv --stdout | pbcopy`）
   - `--copy`: テキストをクリップボードへ
   - `qrsend inbox export <id> -o <dir>`: 受信済みセッションを後から書き出す

### 8.2 Web

1. **Receive 画面**: 背面カメラで読み取る。manifest を復号した時点で、送信者・ファイル一覧・合計サイズ・所要時間の見積もりを表示する。
2. **開始前チェック**:
   - `navigator.storage.estimate()` で空き容量を確認する。
   - `navigator.storage.persist()` を要求し、長時間の受信中にブラウザがデータを消さないようにする。
   - iOS Safari は、操作の無いサイトのデータを一定期間で消すことがあるため、長時間の転送ではホーム画面への追加（PWA）を案内する。
3. **受信中**: 暗号文を **OPFS**（ブラウザ内の大容量ファイル領域。Worker から同期書き込みできる）に保存する。セグメントの完成状況をマップ表示する。タブを閉じても *Inbox* から再開できる。
4. **完了 → Inbox**: 保存する前に、画像・動画・PDF・テキストをプレビューできる。HTML は描画せずテキストとして表示する。必要なファイルだけを選んで保存することもできる。
5. **保存**: Worker 内で「OPFS → age 復号 → zstd 展開 → 切り出し → BLAKE3 検証」をストリーミング処理し、環境に応じた方法で書き出す。

| 環境 | 単一ファイル | 複数ファイル・フォルダ | テキスト |
|---|---|---|---|
| Chrome / Edge（PC） | 保存ダイアログで直接書き込み（`showSaveFilePicker`） | 保存先フォルダを選び、構造ごと直接書き込み（`showDirectoryPicker`） | 表示 ＋ Copy |
| Firefox / Safari（Mac） | ダウンロード | ZIP（必要に応じて ZIP64）でダウンロード | 表示 ＋ Copy |
| Android Chrome | ダウンロード / 共有 | ZIP でダウンロード | 表示 ＋ Copy |
| iOS / iPadOS Safari | 「ファイル」へダウンロード / 共有シート（写真へ保存など） | ZIP でダウンロード | 表示 ＋ Copy |

- 大容量ファイルの「ダウンロード」は、メモリを経由させずに行う。ストリームを OPFS 上のファイルに書き出し、`getFile()` で得たディスク上の File を object URL 経由でダウンロードさせる。一時的に容量を二重に使うため、Service Worker によるストリーミングダウンロードが使える環境ではそちらを優先する（PoC で検証する）。
- 保存が完了したら Inbox から削除するよう促す（設定で自動削除も可）。

## 9. CLI コマンド設計

```
qrsend send [PATH|-]...            ファイル・フォルダ・標準入力を送信
    --to <DEVICE|PUBKEY>           宛先（複数指定可）。信頼済みデバイス名または公開鍵
    --plain                        暗号化しない（明示指定が必要）
    --text <STRING>                テキストを送信
    --display window|terminal|kitty|sixel
                                   表示方法（既定: window、使えなければ terminal）
    --density low|normal|high|max  QR の密度（--qr-version / --ecc で個別指定も可）
    --fps <N>  --grid <1|2x2>      表示速度・同時表示枚数
    --resume <CODE>                欠損セグメントのみ再送
    --no-compress

qrsend recv                        受信
    --camera <N|NAME> | --video <FILE> | --images <DIR>
    -o, --out <DIR>                出力先（既定: カレントディレクトリ）
    --stdout | --copy
    --on-conflict rename|overwrite|skip
    --keep                         完了後も受信データを保持

qrsend inbox [list | show <ID> | export <ID> -o <DIR> | missing <ID> | rm <ID>]
qrsend id [--qr]                   自分の公開鍵と確認コードを表示
qrsend devices [list | add [<PUBKEY> | --scan] | rm <NAME> | rename <OLD> <NEW>]
qrsend completions <SHELL>
```

## 10. Web 画面構成

| 画面 | 内容 |
|---|---|
| Home | 大きなボタン 2 つ: **Send** / **Receive**。進行中の受信があれば Inbox へのバッジ表示 |
| Send | ドロップ・ファイル選択・フォルダ選択・テキスト貼り付け → 宛先選択（Trusted devices / Anyone） → 全画面 QR ループ（速度・密度・グリッドの調整、Wake Lock） |
| Receive | カメラ映像、送信者・中身の表示、セグメントマップ、速度・残り時間 |
| Inbox | 受信中・受信済みセッションの一覧、プレビュー、保存、削除、resume code の表示 |
| Devices | 自分の ID（QR・確認コード）、信頼済みデバイスの追加・削除 |
| Settings | 既定の密度・速度、自動削除、カメラ選択、ID のバックアップ（暗号化エクスポート） |

## 11. 技術スタック

| 領域 | 採用候補 |
|---|---|
| ファウンテン符号 | `raptorq` crate |
| 暗号 | `age` crate、X25519 / Ed25519（RustCrypto 系）、Web は WebCrypto の X25519（non-extractable） |
| ハッシュ | BLAKE3 |
| 圧縮 | zstd（WASM ビルド可否を PoC で確認） |
| アーカイブ | 独自の単純連結形式（境界は manifest のサイズで決まる）。tar の長いパス名・メタデータ問題を避け、検証を manifest に一本化するため |
| QR 生成 | `qrcode` crate（core / CLI）、Web は core の WASM を利用 |
| QR 読取（Web） | BarcodeDetector（使える環境）→ zxing-wasm（フォールバック）、Web Worker で実行 |
| QR 読取（CLI） | `rxing` または zxing-cpp バインディング（PoC で比較） |
| カメラ（CLI） | `nokhwa` 等。動画入力は ffmpeg 連携を検討 |
| 表示（CLI） | 専用ウィンドウ（winit + softbuffer 等）、ターミナルのブロック文字、kitty / sixel |
| Web UI | Svelte 5 + Vite + TypeScript、vite-plugin-pwa |
| Web 保存 | OPFS、File System Access API、Web Share API、Service Worker |

## 12. 配布（GitHub の機能をフル活用）

- **GitHub Pages**: Web 版（PWA）。Actions でテストが通ったら自動デプロイ
- **GitHub Releases**: CLI バイナリ（macOS / Linux / Windows、`cargo-dist`）、エアギャップ用の単一 HTML 版
- **Homebrew tap** / シェルインストーラ（`cargo-dist` が生成）
- **Artifact Attestation**: Release 成果物のビルド元証明

## 13. テスト戦略

- `qrsend-core` の単体テスト・プロパティテスト: フレームをランダムに欠損・重複・並べ替えしても往復で完全に復元できること
- CLI 同士の E2E: `send` が出力したフレーム画像列を `recv --images` に入力して往復させる
- Web の E2E: 送信 QR アニメーションを y4m 動画に書き出し、Chrome の仮想カメラ（`--use-file-for-fake-video-capture`）に入力して Playwright で検証
- 相互運用テスト: CLI → Web、Web → CLI

## 14. ロードマップ

| 段階 | 内容 |
|---|---|
| v0.1 | `qrsend-core`（container・セグメント・RaptorQ・フレーム）と、CLI の `send --display window` / `recv --video` / `recv --images`。大容量の往復が成立するかを検証し、速度を実測 |
| v0.2 | CLI のカメラ受信、Inbox と再開、Web 版の受信（カメラ・OPFS・保存）と送信、CLI ⇄ Web の相互運用 |
| v0.3 | デバイス ID・ペアリング・age 暗号化・署名、resume code による欠損再送 |
| v0.4 | PWA 化、Share Target、単一 HTML 版、Release の署名・Homebrew |
| 将来 | グリッド表示の最適化、カラーの高密度モード、カメラ付き PC での双方向自動調整、QR で SDP を交換する WebRTC ターボモード |

## 15. 未決事項

- セグメントサイズ、シンボルサイズ、QR バージョン・誤り訂正レベルの既定値（実測して決める）
- フレームヘッダの正確なレイアウト（PROTOCOL.md で定義）
- 動画入力の実装方式（ffmpeg への依存を許容するか）
- ID のバックアップと、複数端末間での ID 移行の扱い
