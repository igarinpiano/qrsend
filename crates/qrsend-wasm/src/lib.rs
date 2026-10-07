//! WebAssembly bindings used by the QRSend web app.
//!
//! The app runs these in a Web Worker. Nothing large is kept in WebAssembly
//! memory: packed bodies are written out through a JavaScript callback as
//! they are produced, segments are read back through another callback when
//! frames are generated, received segments are handed to JavaScript to store,
//! and extraction streams from storage to storage. JavaScript backs those
//! callbacks with OPFS files (synchronous access handles).
//!
//! Private keys can stay outside too: with WebCrypto the app holds
//! non-extractable keys, computes X25519 shared secrets and Ed25519
//! signatures itself, and passes only those results in. [`Identity`] is the
//! fallback for browsers without those WebCrypto algorithms (and the source
//! for migrating identities created by earlier versions).

use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};

use js_sys::{Array, Function, Object, Reflect, Uint8Array};
use qrsend_core::crypto::{self, DeviceIdentity, DevicePublic, OpenMetaError, SharedSecrets};
use qrsend_core::fec;
use qrsend_core::frame::{FLAG_ENCRYPTED, Frame, META_INDEX};
use qrsend_core::manifest::{
    Body, Entry, EntryType, Kind, MANIFEST_VERSION, Manifest, MetaEnvelope, MetaSignature,
    session_hex, signed_message,
};
use qrsend_core::payload::{
    BodyDigest, BodyHasher, PackOptions, Packed, Packer, UnpackSink, segment_hash,
};
use qrsend_core::qr::{self, Ec, QrParams};
use qrsend_core::receiver::{Event, Receiver, SessionParams};
use qrsend_core::resume::ResumeCode;
use qrsend_core::sanitize::SafePath;
use qrsend_core::schedule::ScheduleConfig;
use qrsend_core::sender::{SegmentSource, Sender, SessionLayout};
use serde::Serialize;
use wasm_bindgen::prelude::*;

type JsResult<T> = Result<T, JsError>;

const SEG_SHIFT: u8 = 20;

fn js_err(e: impl std::fmt::Display) -> JsError {
    JsError::new(&e.to_string())
}

fn to_js<T: Serialize>(v: &T) -> JsResult<JsValue> {
    v.serialize(&serde_wasm_bindgen::Serializer::json_compatible())
        .map_err(js_err)
}

fn io_err(e: JsValue) -> io::Error {
    io::Error::other(
        e.as_string()
            .or_else(|| js_sys::Error::from(e).message().as_string())
            .unwrap_or_default(),
    )
}

fn key32(bytes: &[u8], what: &str) -> JsResult<[u8; 32]> {
    bytes
        .try_into()
        .map_err(|_| js_err(format!("{what} must be 32 bytes")))
}

fn set(obj: &Object, key: &str, value: impl Into<JsValue>) {
    let _ = Reflect::set(obj, &key.into(), &value.into());
}

#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

fn qr_params(version: u8, ec: &str) -> JsResult<QrParams> {
    if !(1..=40).contains(&version) {
        return Err(js_err("QR version must be 1..=40"));
    }
    Ok(QrParams {
        version,
        ec: ec.parse::<Ec>().map_err(js_err)?,
    })
}

/// Bytes of payload per QR code for this version / error correction level.
#[wasm_bindgen(js_name = symbolSize)]
pub fn symbol_size(version: u8, ec: &str) -> JsResult<usize> {
    Ok(qr_params(version, ec)?.symbol_size())
}

#[derive(Serialize)]
struct ParamsInfo {
    version: u8,
    ec: String,
    modules: usize,
    #[serde(rename = "symbolSize")]
    symbol_size: usize,
}

fn params_info(p: QrParams) -> ParamsInfo {
    ParamsInfo {
        version: p.version,
        ec: format!("{:?}", p.ec),
        modules: p.modules(),
        symbol_size: p.symbol_size(),
    }
}

/// The code size chosen for a transfer of this size: `{ version, ec, modules, symbolSize }`.
#[wasm_bindgen(js_name = autoParams)]
pub fn auto_params(meta_len: u32, body_len: f64) -> JsResult<JsValue> {
    to_js(&params_info(qr::auto_params(meta_len, body_len as u64)))
}

/// Gap between codes in a grid, in modules.
#[wasm_bindgen(js_name = quietModules)]
pub fn quiet_modules() -> usize {
    qr::QUIET
}

/// A rendered QR code: `width × width` modules, 1 = dark.
#[wasm_bindgen]
pub struct QrImage {
    width: usize,
    modules: Vec<u8>,
}

#[wasm_bindgen]
impl QrImage {
    #[wasm_bindgen(getter)]
    pub fn width(&self) -> usize {
        self.width
    }

    #[wasm_bindgen(getter)]
    pub fn modules(&self) -> Uint8Array {
        Uint8Array::from(&self.modules[..])
    }
}

/// Renders arbitrary text (device IDs).
#[wasm_bindgen(js_name = renderText)]
pub fn render_text(text: &str) -> JsResult<QrImage> {
    let m = qr::render_text(text).map_err(js_err)?;
    Ok(QrImage {
        width: m.width,
        modules: m.modules.into_iter().map(u8::from).collect(),
    })
}

/// Decodes QR codes in an RGBA image (fallback when no other detector exists).
#[wasm_bindgen(js_name = detectRgba)]
pub fn detect_rgba(width: usize, height: usize, rgba: &[u8]) -> Array {
    let luma: Vec<u8> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&[r, g, b, _]| ((r as u32 * 77 + g as u32 * 150 + b as u32 * 29) >> 8) as u8)
        .collect();
    qr::detect(qr::Luma {
        width,
        height,
        pixels: &luma,
    })
    .into_iter()
    .map(JsValue::from)
    .collect()
}

// ---------------------------------------------------------------- identity

#[derive(Serialize)]
struct DeviceInfo {
    name: String,
    fingerprint: String,
    id: String,
}

fn device_info(p: &DevicePublic) -> DeviceInfo {
    DeviceInfo {
        name: p.name.clone(),
        fingerprint: p.fingerprint(),
        id: p.to_id_string(),
    }
}

/// Parses a `qrsend-id:…` string: `{ name, fingerprint, id }`.
#[wasm_bindgen(js_name = parseDeviceId)]
pub fn parse_device_id(id: &str) -> JsResult<JsValue> {
    to_js(&device_info(&DevicePublic::parse(id).map_err(js_err)?))
}

/// `{ name, fingerprint, id }` of a device given its raw public keys
/// (identities whose private keys live in WebCrypto).
#[wasm_bindgen(js_name = deviceInfo)]
pub fn device_info_from_keys(
    name: &str,
    x25519_public: &[u8],
    ed25519_public: &[u8],
) -> JsResult<JsValue> {
    let p = DevicePublic::from_raw(
        name,
        &key32(x25519_public, "X25519 key")?,
        &key32(ed25519_public, "Ed25519 key")?,
    )
    .map_err(js_err)?;
    to_js(&device_info(&p))
}

/// Ephemeral public keys of the X25519 stanzas of an age file (its header is
/// enough). One shared secret per key is needed to decrypt it.
#[wasm_bindgen(js_name = stanzaKeys)]
pub fn stanza_keys(age_bytes: &[u8]) -> JsResult<Array> {
    Ok(crypto::x25519_stanza_keys(age_bytes)
        .map_err(js_err)?
        .iter()
        .map(|k| JsValue::from(Uint8Array::from(&k[..])))
        .collect())
}

/// A device identity whose private keys are held in WebAssembly memory.
/// Only for browsers without WebCrypto X25519 / Ed25519, and for migrating
/// identities stored by earlier versions.
#[wasm_bindgen]
pub struct Identity {
    inner: DeviceIdentity,
}

#[wasm_bindgen]
impl Identity {
    pub fn generate(name: &str) -> JsResult<Identity> {
        Ok(Identity {
            inner: DeviceIdentity::generate(name).map_err(js_err)?,
        })
    }

    pub fn parse(secret: &str) -> JsResult<Identity> {
        Ok(Identity {
            inner: DeviceIdentity::parse(secret).map_err(js_err)?,
        })
    }

    /// Secret key material (store privately).
    pub fn secret(&self) -> String {
        self.inner.to_secret_string()
    }

    pub fn rename(&mut self, name: &str) {
        self.inner.name = name.to_string();
    }

    /// `{ name, fingerprint, id }`
    pub fn info(&self) -> JsResult<JsValue> {
        to_js(&device_info(&self.inner.public()))
    }

    /// `{ x25519Secret, x25519Public, ed25519Seed, ed25519Public }` as byte
    /// arrays, for importing into WebCrypto.
    #[wasm_bindgen(js_name = rawKeys)]
    pub fn raw_keys(&self) -> Object {
        let raw = self.inner.raw_keys();
        let o = Object::new();
        set(&o, "x25519Secret", Uint8Array::from(&raw.x25519_secret[..]));
        set(&o, "x25519Public", Uint8Array::from(&raw.x25519_public[..]));
        set(&o, "ed25519Seed", Uint8Array::from(&raw.ed25519_seed[..]));
        set(
            &o,
            "ed25519Public",
            Uint8Array::from(&raw.ed25519_public[..]),
        );
        o
    }

    /// Signs the message returned by [`SendJob::seal`].
    pub fn sign(&self, message: &[u8]) -> Uint8Array {
        Uint8Array::from(&self.inner.sign_bytes(message)[..])
    }
}

/// The key used to decrypt: held here (legacy) or represented by shared
/// secrets computed by WebCrypto.
enum Keys {
    None,
    Legacy(Box<DeviceIdentity>),
    Shared(SharedSecrets),
}

impl Keys {
    fn as_age(&self) -> Option<&dyn crypto::AgeIdentity> {
        match self {
            Keys::None => None,
            Keys::Legacy(id) => Some(id.as_age()),
            Keys::Shared(s) => Some(s),
        }
    }
}

// ---------------------------------------------------------------- sending

/// Writes body bytes to JavaScript (`write(Uint8Array)`), which appends them
/// to the spool file.
struct JsWriter(Function);

impl Write for JsWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .call1(&JsValue::NULL, &Uint8Array::from(buf))
            .map_err(io_err)?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

type Spool = BodyHasher<BufWriter<JsWriter>>;

enum BodySink {
    Plain(Spool),
    Encrypted(crypto::EncryptWriter<Spool>),
}

impl Write for BodySink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            BodySink::Plain(w) => w.write(buf),
            BodySink::Encrypted(w) => w.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            BodySink::Plain(w) => w.flush(),
            BodySink::Encrypted(w) => w.flush(),
        }
    }
}

impl BodySink {
    fn finish(self) -> io::Result<BodyDigest> {
        let spool = match self {
            BodySink::Plain(w) => w,
            BodySink::Encrypted(w) => w.finish()?,
        };
        let (mut out, digest) = spool.finish()?;
        out.flush()?;
        Ok(digest)
    }
}

fn pack_options() -> PackOptions {
    PackOptions {
        zstd_level: Some(3),
        zstd_workers: 0,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SendInfo {
    session: String,
    flags: u8,
    seg_shift: u8,
    meta_len: u32,
    body_len: f64,
    summary: String,
    encrypted: bool,
}

/// Packs a transfer. Usage: configure recipients, add entries (file contents
/// in chunks) or a text, `seal()`, optionally sign, `finish()`.
#[wasm_bindgen]
pub struct SendJob {
    session_id: u32,
    write: Option<Function>,
    recipients: Vec<DevicePublic>,
    sender_name: Option<String>,
    packer: Option<Packer<BodySink>>,
    text: Option<String>,
    sealed: Option<(Manifest, MetaEnvelope)>,
    info: Option<SendInfo>,
}

#[wasm_bindgen]
impl SendJob {
    /// `write(Uint8Array)` receives the body bytes in order.
    #[wasm_bindgen(constructor)]
    pub fn new(write: Function) -> JsResult<SendJob> {
        let mut id = [0u8; 4];
        getrandom::getrandom(&mut id).map_err(js_err)?;
        Ok(SendJob {
            session_id: u32::from_le_bytes(id).max(1),
            write: Some(write),
            recipients: Vec::new(),
            sender_name: None,
            packer: None,
            text: None,
            sealed: None,
            info: None,
        })
    }

    /// Encrypt for this device ID. Call before adding content.
    #[wasm_bindgen(js_name = addRecipient)]
    pub fn add_recipient(&mut self, device_id: &str) -> JsResult<()> {
        if self.packer.is_some() {
            return Err(js_err("recipients must be added before content"));
        }
        self.recipients
            .push(DevicePublic::parse(device_id).map_err(js_err)?);
        Ok(())
    }

    #[wasm_bindgen(js_name = setSenderName)]
    pub fn set_sender_name(&mut self, name: &str) {
        self.sender_name = Some(name.to_string());
    }

    fn sink(&mut self) -> JsResult<BodySink> {
        let write = self
            .write
            .take()
            .ok_or_else(|| js_err("job already sealed"))?;
        let spool = BodyHasher::new(
            BufWriter::with_capacity(1 << 20, JsWriter(write)),
            SEG_SHIFT,
        );
        Ok(if self.recipients.is_empty() {
            BodySink::Plain(spool)
        } else {
            BodySink::Encrypted(crypto::encrypt_writer(spool, &self.recipients).map_err(js_err)?)
        })
    }

    fn packer(&mut self) -> JsResult<&mut Packer<BodySink>> {
        if self.packer.is_none() {
            let sink = self.sink()?;
            self.packer = Some(Packer::new(sink, pack_options()).map_err(js_err)?);
        }
        Ok(self.packer.as_mut().unwrap())
    }

    #[wasm_bindgen(js_name = addDir)]
    pub fn add_dir(&mut self, path: &str) -> JsResult<()> {
        self.packer()?.add_dir(path, None);
        Ok(())
    }

    /// `mtime_ms` as `File.lastModified`.
    #[wasm_bindgen(js_name = beginFile)]
    pub fn begin_file(&mut self, path: &str, mtime_ms: Option<f64>) -> JsResult<()> {
        self.packer()?
            .begin_file(path, None, mtime_ms.map(|ms| (ms / 1000.0) as i64));
        Ok(())
    }

    #[wasm_bindgen(js_name = writeChunk)]
    pub fn write_chunk(&mut self, chunk: &[u8]) -> JsResult<()> {
        self.packer()?.write_chunk(chunk).map_err(js_err)
    }

    /// `size` is the size announced for the file; a mismatch is an error.
    #[wasm_bindgen(js_name = endFile)]
    pub fn end_file(&mut self, size: f64) -> JsResult<()> {
        self.packer()?.end_file(Some(size as u64)).map_err(js_err)?;
        Ok(())
    }

    /// Send a text instead of files.
    #[wasm_bindgen(js_name = setText)]
    pub fn set_text(&mut self, text: &str) {
        self.text = Some(text.to_string());
    }

    /// Finishes the body and builds the manifest. Returns the message a
    /// sender identity should sign (pass the signature to `finish`).
    pub fn seal(&mut self) -> JsResult<Uint8Array> {
        let (sink, packed): (BodySink, Packed) = match self.text.take() {
            Some(text) => {
                let sink = self.sink()?;
                Packer::text(sink, &text, pack_options()).map_err(js_err)?
            }
            None => {
                self.packer()?;
                self.packer.take().unwrap().finish().map_err(js_err)?
            }
        };
        let digest = sink.finish().map_err(js_err)?;
        let manifest = Manifest {
            qrsend: MANIFEST_VERSION,
            session: session_hex(self.session_id),
            kind: packed.kind,
            created: (js_sys::Date::now() / 1000.0) as i64,
            sender_name: self.sender_name.clone(),
            body: Body {
                length: digest.length,
                blake3: digest.blake3,
                segment_blake3: digest.segment_blake3,
                encoding: packed.encoding,
                plain_length: packed.plain_length,
            },
            entries: packed.entries,
        };
        let envelope = MetaEnvelope::from_manifest(&manifest).map_err(js_err)?;
        let message = signed_message(self.session_id, &envelope.manifest_z);
        self.sealed = Some((manifest, envelope));
        Ok(Uint8Array::from(&message[..]))
    }

    /// Returns the meta segment. `signature` (64 bytes) and `signer` (the
    /// 32-byte Ed25519 public key) sign the manifest when given.
    pub fn finish(
        &mut self,
        signature: Option<Vec<u8>>,
        signer: Option<Vec<u8>>,
    ) -> JsResult<Uint8Array> {
        let (manifest, mut envelope) = self.sealed.take().ok_or_else(|| js_err("seal() first"))?;
        if let (Some(sig), Some(pk)) = (signature, signer) {
            envelope.signature = Some(MetaSignature {
                signer: key32(&pk, "signer key")?,
                signature: sig
                    .try_into()
                    .map_err(|_| js_err("signature must be 64 bytes"))?,
            });
        }
        let mut meta = envelope.encode();
        let mut flags = 0;
        if !self.recipients.is_empty() {
            meta = crypto::encrypt(&meta, &self.recipients).map_err(js_err)?;
            flags |= FLAG_ENCRYPTED;
        }
        self.info = Some(SendInfo {
            session: session_hex(self.session_id),
            flags,
            seg_shift: SEG_SHIFT,
            meta_len: meta.len() as u32,
            body_len: manifest.body.length as f64,
            summary: summary(&manifest),
            encrypted: flags != 0,
        });
        Ok(Uint8Array::from(&meta[..]))
    }

    /// `{ session, flags, segShift, metaLen, bodyLen, summary, encrypted }` after `finish`.
    pub fn info(&self) -> JsResult<JsValue> {
        to_js(self.info.as_ref().ok_or_else(|| js_err("finish() first"))?)
    }
}

fn summary(m: &Manifest) -> String {
    match m.kind {
        Kind::Text => format!("Text ({} bytes)", m.body.plain_length),
        Kind::Files => {
            let tops: Vec<&str> = m
                .entries
                .iter()
                .filter(|e| !e.path.contains('/'))
                .map(|e| e.path.as_str())
                .collect();
            let files = m.file_count();
            let names = match tops.len() {
                0 => "(empty)".to_string(),
                1 => tops[0].to_string(),
                n => format!("{} and {} more", tops[0], n - 1),
            };
            format!(
                "{names} · {files} file{}",
                if files == 1 { "" } else { "s" }
            )
        }
    }
}

/// Reads segments from JavaScript (`read(index) -> Uint8Array`; 0 is meta).
struct JsSource(Function);

impl SegmentSource for JsSource {
    fn segment(&mut self, index: u32) -> io::Result<Vec<u8>> {
        let value = self
            .0
            .call1(&JsValue::NULL, &index.into())
            .map_err(io_err)?;
        Ok(Uint8Array::new(&value).to_vec())
    }
}

/// An endless stream of QR codes for one packed transfer.
#[wasm_bindgen]
pub struct SendSession {
    sender: Sender<JsSource>,
    params: QrParams,
    frames: u64,
}

#[wasm_bindgen]
impl SendSession {
    /// `read(index) -> Uint8Array` returns a segment (0 = meta). `qr_version`
    /// 0 picks the code size from the transfer size.
    #[wasm_bindgen(constructor)]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        session: &str,
        flags: u8,
        seg_shift: u8,
        meta_len: u32,
        body_len: f64,
        read: Function,
        qr_version: u8,
        ec: &str,
        redundancy: f64,
    ) -> JsResult<SendSession> {
        let params = if qr_version == 0 {
            qr::auto_params(meta_len, body_len as u64)
        } else {
            qr_params(qr_version, ec)?
        };
        let symbol_size = params.symbol_size();
        if meta_len as u64 > fec::max_segment_len(symbol_size) {
            return Err(js_err("the file list is too large for this QR density"));
        }
        let layout = SessionLayout {
            session_id: u32::from_str_radix(session, 16).map_err(js_err)?,
            flags,
            seg_shift,
            meta_len,
            body_len: body_len as u64,
            symbol_size,
        };
        let config = ScheduleConfig {
            redundancy,
            ..ScheduleConfig::default()
        };
        Ok(SendSession {
            sender: Sender::new(layout, JsSource(read), config, None),
            params,
            frames: 0,
        })
    }

    /// The next `n` codes as one array of `n × width × width` modules (1 = dark).
    #[wasm_bindgen(js_name = nextBatch)]
    pub fn next_batch(&mut self, n: usize) -> JsResult<Uint8Array> {
        let w = self.params.modules();
        let mut out = Vec::with_capacity(n * w * w);
        for _ in 0..n {
            let frame = self.sender.next_frame().map_err(js_err)?;
            let m = qr::render(&frame.to_qr_text(), self.params).map_err(js_err)?;
            out.extend(m.modules.into_iter().map(u8::from));
        }
        self.frames += n as u64;
        Ok(Uint8Array::from(&out[..]))
    }

    /// The next frame as QR text (for tests).
    #[wasm_bindgen(js_name = nextText)]
    pub fn next_text(&mut self) -> JsResult<String> {
        self.frames += 1;
        Ok(self.sender.next_frame().map_err(js_err)?.to_qr_text())
    }

    /// `{ version, ec, modules, symbolSize }`
    pub fn params(&self) -> JsResult<JsValue> {
        to_js(&params_info(self.params))
    }

    #[wasm_bindgen(getter, js_name = framesPerPass)]
    pub fn frames_per_pass(&self) -> f64 {
        self.sender.frames_per_pass() as f64
    }

    #[wasm_bindgen(getter)]
    pub fn frames(&self) -> f64 {
        self.frames as f64
    }

    #[wasm_bindgen(getter)]
    pub fn pass(&self) -> f64 {
        self.sender.pass() as f64
    }
}

// ---------------------------------------------------------------- receiving

#[derive(Serialize, Default)]
struct PushResult {
    /// Session id when this frame locked the receiver.
    locked: Option<String>,
    foreign: Option<String>,
    /// Segments completed by this push (data available via `takeCompleted`).
    completed: Vec<u32>,
    /// Segments that failed verification and will be received again.
    rejected: Vec<u32>,
}

#[derive(Serialize)]
struct EntryInfo {
    path: String,
    dir: bool,
    size: f64,
}

#[derive(Serialize)]
struct Info {
    session: Option<String>,
    encrypted: bool,
    total: usize,
    done: usize,
    complete: bool,
    kind: Option<&'static str>,
    summary: Option<String>,
    plain_length: Option<f64>,
    wire_length: Option<f64>,
    sender_name: Option<String>,
    /// "trusted" | "unverified" | "unsigned"
    sender_status: Option<&'static str>,
    sender: Option<String>,
    entries: Vec<EntryInfo>,
    frames: f64,
    useful: f64,
}

/// One incoming transfer. Segment data is not kept here: completed segments
/// are taken with `takeCompleted` and stored by the caller.
#[wasm_bindgen]
pub struct Receive {
    rx: Receiver,
    keys: Keys,
    trusted: Vec<DevicePublic>,
    manifest: Option<Manifest>,
    signer: Option<[u8; 32]>,
    completed: Vec<(u32, Vec<u8>)>,
}

#[wasm_bindgen]
impl Receive {
    /// `session`: only accept this session id (hex), e.g. when resuming.
    #[wasm_bindgen(constructor)]
    pub fn new(session: Option<String>) -> JsResult<Receive> {
        let mut rx = Receiver::new();
        if let Some(s) = session {
            rx = rx.expect_session(u32::from_str_radix(&s, 16).map_err(js_err)?);
        }
        Ok(Receive {
            rx,
            keys: Keys::None,
            trusted: Vec::new(),
            manifest: None,
            signer: None,
            completed: Vec::new(),
        })
    }

    /// Decrypt with an identity held in WebAssembly (legacy).
    #[wasm_bindgen(js_name = setIdentity)]
    pub fn set_identity(&mut self, identity: &Identity) -> JsResult<()> {
        self.keys = Keys::Legacy(Box::new(
            DeviceIdentity::parse(&identity.inner.to_secret_string()).map_err(js_err)?,
        ));
        Ok(())
    }

    /// Decrypt with shared secrets computed elsewhere: first this device's
    /// X25519 public key, then one `addSecret` per stanza key.
    #[wasm_bindgen(js_name = setPublicKey)]
    pub fn set_public_key(&mut self, x25519_public: &[u8]) -> JsResult<()> {
        self.keys = Keys::Shared(SharedSecrets {
            our_public: key32(x25519_public, "X25519 key")?,
            secrets: Vec::new(),
        });
        Ok(())
    }

    #[wasm_bindgen(js_name = addSecret)]
    pub fn add_secret(&mut self, ephemeral_key: &[u8], shared_secret: &[u8]) -> JsResult<()> {
        let Keys::Shared(s) = &mut self.keys else {
            return Err(js_err("setPublicKey() first"));
        };
        s.secrets.push((
            key32(ephemeral_key, "ephemeral key")?,
            key32(shared_secret, "shared secret")?,
        ));
        Ok(())
    }

    /// Trusted device IDs (with display names) for sender verification.
    #[wasm_bindgen(js_name = addTrusted)]
    pub fn add_trusted(&mut self, device_id: &str, name: &str) -> JsResult<()> {
        let mut p = DevicePublic::parse(device_id).map_err(js_err)?;
        p.name = name.to_string();
        self.trusted.push(p);
        Ok(())
    }

    /// Restores a saved session's parameters; then `markDone` what is stored.
    pub fn restore(
        &mut self,
        session: &str,
        flags: u8,
        seg_shift: u8,
        seg_count: u32,
    ) -> JsResult<()> {
        let session_id = u32::from_str_radix(session, 16).map_err(js_err)?;
        self.rx.restore(SessionParams {
            session_id,
            flags,
            seg_shift,
            seg_count,
        });
        Ok(())
    }

    #[wasm_bindgen(js_name = markDone)]
    pub fn mark_done(&mut self, index: u32) {
        self.rx.mark_done(index);
    }

    /// Forget a stored segment (it failed verification).
    #[wasm_bindgen(js_name = resetSegment)]
    pub fn reset_segment(&mut self, index: u32) {
        self.rx.reset(index);
    }

    /// Session parameters `{ session, flags, segShift, segCount }` once locked.
    pub fn params(&self) -> JsResult<JsValue> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct P {
            session: String,
            flags: u8,
            seg_shift: u8,
            seg_count: u32,
        }
        match self.rx.params() {
            None => Ok(JsValue::NULL),
            Some(p) => to_js(&P {
                session: session_hex(p.session_id),
                flags: p.flags,
                seg_shift: p.seg_shift,
                seg_count: p.seg_count,
            }),
        }
    }

    /// Feeds one decoded QR text. Completed segments (including meta, index
    /// 0) are queued for `takeCompleted`; body segments are verified here
    /// when the manifest is already known.
    pub fn push(&mut self, text: &str) -> JsResult<JsValue> {
        let mut out = PushResult::default();
        let Ok(frame) = Frame::from_qr_text(text) else {
            return to_js(&out);
        };
        for ev in self.rx.push(frame) {
            match ev {
                Event::Locked(p) => out.locked = Some(session_hex(p.session_id)),
                Event::ForeignSession(id) => out.foreign = Some(session_hex(id)),
                Event::Inconsistent => {}
                Event::Completed { index, data } => {
                    if index != META_INDEX && !self.matches_manifest(index, &data) {
                        self.rx.reset(index);
                        out.rejected.push(index);
                    } else {
                        out.completed.push(index);
                        self.completed.push((index, data));
                    }
                }
            }
        }
        to_js(&out)
    }

    /// Completed segments since the last call: `[{ index, data }]` (to store).
    #[wasm_bindgen(js_name = takeCompleted)]
    pub fn take_completed(&mut self) -> Array {
        self.completed
            .drain(..)
            .map(|(index, data)| {
                let o = Object::new();
                set(&o, "index", index);
                set(&o, "data", Uint8Array::from(&data[..]));
                JsValue::from(o)
            })
            .collect()
    }

    #[wasm_bindgen(js_name = hasManifest)]
    pub fn has_manifest(&self) -> bool {
        self.manifest.is_some()
    }

    /// Decrypts and checks the meta segment. For encrypted transfers the keys
    /// must have been provided (`setIdentity`, or `setPublicKey` + one
    /// `addSecret` per `stanzaKeys(meta)`).
    #[wasm_bindgen(js_name = openMeta)]
    pub fn open_meta(&mut self, meta: &[u8]) -> JsResult<()> {
        let p = *self.rx.params().ok_or_else(|| js_err("no session"))?;
        let encrypted = p.flags & FLAG_ENCRYPTED != 0;
        let opened =
            crypto::open_meta(meta, p.session_id, encrypted, self.keys.as_age()).map_err(|e| {
                match e {
                    OpenMetaError::NoIdentity => {
                        js_err("This transfer is encrypted. Create your device ID first.")
                    }
                    OpenMetaError::NotForUs => {
                        js_err("This transfer is encrypted for another device.")
                    }
                    e => js_err(e),
                }
            })?;
        let m = opened.manifest;
        if m.body.length.div_ceil(1 << p.seg_shift) != p.seg_count as u64
            || m.body.segment_blake3.len() != p.seg_count as usize
        {
            return Err(js_err("manifest does not match the transfer layout"));
        }
        self.manifest = Some(m);
        self.signer = opened.signer;
        Ok(())
    }

    fn matches_manifest(&self, index: u32, data: &[u8]) -> bool {
        match &self.manifest {
            Some(m) => m
                .body
                .segment_blake3
                .get(index as usize - 1)
                .is_some_and(|h| *h == segment_hash(data)),
            None => true,
        }
    }

    /// Whether a stored body segment matches the manifest (true while the
    /// manifest is unknown). For segments stored before the manifest arrived.
    #[wasm_bindgen(js_name = verifySegment)]
    pub fn verify_segment(&self, index: u32, data: &[u8]) -> bool {
        index != META_INDEX && self.matches_manifest(index, data)
    }

    /// Byte length of a body segment according to the manifest.
    #[wasm_bindgen(js_name = segmentLength)]
    pub fn segment_length(&self, index: u32) -> JsResult<f64> {
        let (m, p) = self.manifest_and_params()?;
        let nominal = 1u64 << p.seg_shift;
        let start = (index as u64 - 1) * nominal;
        Ok(m.body.length.saturating_sub(start).min(nominal) as f64)
    }

    fn manifest_and_params(&self) -> JsResult<(&Manifest, SessionParams)> {
        Ok((
            self.manifest
                .as_ref()
                .ok_or_else(|| js_err("the file list has not arrived yet"))?,
            *self.rx.params().ok_or_else(|| js_err("no session"))?,
        ))
    }

    pub fn info(&self) -> JsResult<JsValue> {
        let p = self.rx.params();
        let m = self.manifest.as_ref();
        let (sender_status, sender) = match (m, self.signer) {
            (None, _) => (None, None),
            (Some(_), None) => (Some("unsigned"), None),
            (Some(_), Some(key)) => {
                match self.trusted.iter().find(|t| t.verifying.to_bytes() == key) {
                    Some(t) => (Some("trusted"), Some(t.name.clone())),
                    None => (Some("unverified"), Some(crypto::key_id(&key))),
                }
            }
        };
        let (frames, useful, _) = self.rx.stats();
        to_js(&Info {
            session: p.map(|p| session_hex(p.session_id)),
            encrypted: p.is_some_and(|p| p.flags & FLAG_ENCRYPTED != 0),
            total: self.rx.segment_total(),
            done: self.rx.completed_count(),
            complete: self.is_complete(),
            kind: m.map(|m| match m.kind {
                Kind::Text => "text",
                Kind::Files => "files",
            }),
            summary: m.map(summary),
            plain_length: m.map(|m| m.body.plain_length as f64),
            wire_length: m.map(|m| m.body.length as f64),
            sender_name: m.and_then(|m| m.sender_name.clone()),
            sender_status,
            sender,
            entries: m
                .map(|m| {
                    m.entries
                        .iter()
                        .map(|e| EntryInfo {
                            path: e.path.clone(),
                            dir: e.kind == EntryType::Dir,
                            size: e.file_size() as f64,
                        })
                        .collect()
                })
                .unwrap_or_default(),
            frames: frames as f64,
            useful: useful as f64,
        })
    }

    /// All segments received and the manifest opened. (The caller must also
    /// have verified segments it stored before the manifest arrived.)
    #[wasm_bindgen(js_name = isComplete)]
    pub fn is_complete(&self) -> bool {
        self.rx.is_complete() && self.manifest.is_some()
    }

    /// Resume code for the segments still missing.
    #[wasm_bindgen(js_name = resumeCode)]
    pub fn resume_code(&self) -> Option<String> {
        let p = self.rx.params()?;
        let missing = self.rx.missing();
        (!missing.is_empty()).then(|| ResumeCode::new(p.session_id, missing).encode())
    }

    /// Streams the stored body through decryption, decompression and
    /// unpacking, verifying everything on the way.
    ///
    /// `read(index) -> Uint8Array` returns body segment `index` (1-based).
    /// `sink` receives `dir(path)`, `begin(path, size, mtimeMs | undefined)`,
    /// `write(Uint8Array)`, `end()` for files, or `text(string)` for a text.
    /// For encrypted transfers the body's own stanza keys
    /// (`stanzaKeys(first segment)`) must have been added.
    /// Returns "text" or "files".
    pub fn extract(&self, read: Function, sink: Object) -> JsResult<String> {
        let (m, p) = self.manifest_and_params()?;
        if !self.rx.is_complete() {
            return Err(js_err("not complete"));
        }
        // The whole body once more against the sender's checksum.
        let mut hasher = blake3::Hasher::new();
        io::copy(
            &mut SegmentReader::new(read.clone(), p.seg_count),
            &mut hasher,
        )
        .map_err(js_err)?;
        if hasher.finalize().to_hex().as_str() != m.body.blake3 {
            return Err(js_err("received data does not match the sender's checksum"));
        }
        let body = BufReader::with_capacity(1 << 20, SegmentReader::new(read, p.seg_count));
        let payload: Box<dyn BufRead> = if p.flags & FLAG_ENCRYPTED != 0 {
            let keys = self
                .keys
                .as_age()
                .ok_or_else(|| js_err("no identity to decrypt with"))?;
            Box::new(BufReader::with_capacity(
                1 << 20,
                crypto::decrypt_reader(body, keys).map_err(js_err)?,
            ))
        } else {
            Box::new(body)
        };
        match m.kind {
            Kind::Text => {
                let text = qrsend_core::payload::unpack_text(m, payload).map_err(js_err)?;
                call(&sink, "text", &[text.into()]).map_err(js_err)?;
                Ok("text".into())
            }
            Kind::Files => {
                qrsend_core::payload::unpack(m, payload, &mut JsSink(sink)).map_err(js_err)?;
                Ok("files".into())
            }
        }
    }
}

/// Reads body segments 1..=count in order through a JavaScript callback.
struct SegmentReader {
    read: Function,
    next: u32,
    count: u32,
    current: Vec<u8>,
    pos: usize,
}

impl SegmentReader {
    fn new(read: Function, count: u32) -> Self {
        SegmentReader {
            read,
            next: 1,
            count,
            current: Vec::new(),
            pos: 0,
        }
    }
}

impl Read for SegmentReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.pos == self.current.len() {
            if self.next > self.count {
                return Ok(0);
            }
            let value = self
                .read
                .call1(&JsValue::NULL, &self.next.into())
                .map_err(io_err)?;
            self.current = Uint8Array::new(&value).to_vec();
            self.pos = 0;
            self.next += 1;
        }
        let n = buf.len().min(self.current.len() - self.pos);
        buf[..n].copy_from_slice(&self.current[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

fn call(target: &Object, method: &str, args: &[JsValue]) -> io::Result<()> {
    let f: Function = Reflect::get(target, &method.into()).map_err(io_err)?.into();
    let args: Array = args.iter().collect();
    f.apply(target, &args).map_err(io_err)?;
    Ok(())
}

struct JsSink(Object);

impl UnpackSink for JsSink {
    fn dir(&mut self, path: &SafePath, _: &Entry) -> io::Result<()> {
        call(&self.0, "dir", &[path.to_string().into()])
    }

    fn file(&mut self, path: &SafePath, entry: &Entry, content: &mut dyn Read) -> io::Result<()> {
        let mtime = entry
            .mtime
            .map(|t| JsValue::from(t as f64 * 1000.0))
            .unwrap_or(JsValue::UNDEFINED);
        call(
            &self.0,
            "begin",
            &[
                path.to_string().into(),
                (entry.file_size() as f64).into(),
                mtime,
            ],
        )?;
        let mut buf = vec![0u8; 1 << 18];
        loop {
            let n = content.read(&mut buf)?;
            if n == 0 {
                break;
            }
            call(&self.0, "write", &[Uint8Array::from(&buf[..n]).into()])?;
        }
        call(&self.0, "end", &[])
    }
}
