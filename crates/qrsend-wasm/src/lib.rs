//! WebAssembly bindings used by the QRSend web app.
//!
//! Everything stays in memory: the browser hands over file contents, gets QR
//! module matrices back for display, and feeds decoded QR texts into a
//! [`Receive`] session, persisting completed segments itself (IndexedDB).

use std::collections::BTreeMap;
use std::io::Write;

use js_sys::{Array, Uint8Array};
use qrsend_core::crypto::{self, DeviceIdentity, DevicePublic, OpenMetaError};
use qrsend_core::fec;
use qrsend_core::frame::{FLAG_ENCRYPTED, Frame};
use qrsend_core::manifest::{
    Body, Entry, EntryType, Kind, MANIFEST_VERSION, Manifest, MetaEnvelope, session_hex,
};
use qrsend_core::payload::{BodyHasher, PackOptions, Packed, Packer, UnpackSink, segment_hash};
use qrsend_core::qr::{self, Ec, QrParams};
use qrsend_core::receiver::{Event, Receiver, SessionParams};
use qrsend_core::resume::ResumeCode;
use qrsend_core::sanitize::SafePath;
use qrsend_core::schedule::ScheduleConfig;
use qrsend_core::sender::{MemorySource, Sender, SessionLayout};
use serde::Serialize;
use wasm_bindgen::prelude::*;

type JsResult<T> = Result<T, JsError>;

fn js_err(e: impl std::fmt::Display) -> JsError {
    JsError::new(&e.to_string())
}

fn to_js<T: Serialize>(v: &T) -> JsResult<JsValue> {
    v.serialize(&serde_wasm_bindgen::Serializer::json_compatible())
        .map_err(js_err)
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

impl From<qr::QrMatrix> for QrImage {
    fn from(m: qr::QrMatrix) -> Self {
        QrImage {
            width: m.width,
            modules: m.modules.into_iter().map(u8::from).collect(),
        }
    }
}

/// Renders arbitrary text (device IDs).
#[wasm_bindgen(js_name = renderText)]
pub fn render_text(text: &str) -> JsResult<QrImage> {
    Ok(qr::render_text(text).map_err(js_err)?.into())
}

/// Decodes QR codes in an RGBA image (fallback when no native detector exists).
#[wasm_bindgen(js_name = detectRgba)]
pub fn detect_rgba(width: usize, height: usize, rgba: &[u8]) -> Array {
    let luma: Vec<u8> = rgba
        .chunks_exact(4)
        .map(|p| ((p[0] as u32 * 77 + p[1] as u32 * 150 + p[2] as u32 * 29) >> 8) as u8)
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

/// This browser's device identity.
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
}

/// Parses a `qrsend-id:…` string: `{ name, fingerprint, id }`.
#[wasm_bindgen(js_name = parseDeviceId)]
pub fn parse_device_id(id: &str) -> JsResult<JsValue> {
    to_js(&device_info(&DevicePublic::parse(id).map_err(js_err)?))
}

// ---------------------------------------------------------------- sending

/// Collects what to send, then builds a [`SendSession`].
#[wasm_bindgen]
pub struct SendBuilder {
    packer: Option<Packer<Vec<u8>>>,
    text: Option<String>,
    recipients: Vec<DevicePublic>,
    sender_name: Option<String>,
}

#[wasm_bindgen]
impl SendBuilder {
    #[wasm_bindgen(constructor)]
    pub fn new() -> JsResult<SendBuilder> {
        let packer = Packer::new(
            Vec::new(),
            PackOptions {
                zstd_level: Some(3),
                zstd_workers: 0,
            },
        )
        .map_err(js_err)?;
        Ok(SendBuilder {
            packer: Some(packer),
            text: None,
            recipients: Vec::new(),
            sender_name: None,
        })
    }

    #[wasm_bindgen(js_name = addDir)]
    pub fn add_dir(&mut self, path: &str) {
        if let Some(p) = &mut self.packer {
            p.add_dir(path, None);
        }
    }

    /// `mtime` is in milliseconds (as `File.lastModified`).
    #[wasm_bindgen(js_name = addFile)]
    pub fn add_file(&mut self, path: &str, data: &[u8], mtime_ms: Option<f64>) -> JsResult<()> {
        let p = self
            .packer
            .as_mut()
            .ok_or_else(|| js_err("builder already used"))?;
        let mtime = mtime_ms.map(|ms| (ms / 1000.0) as i64);
        p.add_file(path, &mut &data[..], Some(data.len() as u64), None, mtime)
            .map_err(js_err)?;
        Ok(())
    }

    #[wasm_bindgen(js_name = setText)]
    pub fn set_text(&mut self, text: &str) {
        self.text = Some(text.to_string());
    }

    #[wasm_bindgen(js_name = addRecipient)]
    pub fn add_recipient(&mut self, device_id: &str) -> JsResult<()> {
        self.recipients
            .push(DevicePublic::parse(device_id).map_err(js_err)?);
        Ok(())
    }

    #[wasm_bindgen(js_name = setSenderName)]
    pub fn set_sender_name(&mut self, name: &str) {
        self.sender_name = Some(name.to_string());
    }

    /// Packs everything. `signer` signs the manifest when given.
    pub fn build(
        &mut self,
        signer: Option<Identity>,
        qr_version: u8,
        ec: &str,
        redundancy: f64,
    ) -> JsResult<SendSession> {
        let params = qr_params(qr_version, ec)?;
        let packer = self
            .packer
            .take()
            .ok_or_else(|| js_err("builder already used"))?;
        let seg_shift = 20;
        let mut id = [0u8; 4];
        getrandom::getrandom(&mut id).map_err(js_err)?;
        let session_id = u32::from_le_bytes(id).max(1);

        let (payload, packed): (Vec<u8>, Packed) = match self.text.take() {
            Some(text) => {
                drop(packer);
                Packer::text(
                    Vec::new(),
                    &text,
                    PackOptions {
                        zstd_level: Some(3),
                        zstd_workers: 0,
                    },
                )
                .map_err(js_err)?
            }
            None => packer.finish().map_err(js_err)?,
        };
        let body = if self.recipients.is_empty() {
            payload
        } else {
            crypto::encrypt(&payload, &self.recipients).map_err(js_err)?
        };
        let mut hasher = BodyHasher::new(Vec::new(), seg_shift);
        hasher.write_all(&body).map_err(js_err)?;
        let (body, digest) = hasher.finish().map_err(js_err)?;

        let manifest = Manifest {
            qrsend: MANIFEST_VERSION,
            session: session_hex(session_id),
            kind: packed.kind,
            created: (js_sys::Date::now() / 1000.0) as i64,
            sender_name: self
                .sender_name
                .clone()
                .or_else(|| signer.as_ref().map(|s| s.inner.name.clone())),
            body: Body {
                length: digest.length,
                blake3: digest.blake3,
                segment_blake3: digest.segment_blake3,
                encoding: packed.encoding,
                plain_length: packed.plain_length,
            },
            entries: packed.entries,
        };
        let mut envelope = MetaEnvelope::from_manifest(&manifest).map_err(js_err)?;
        if let Some(s) = &signer {
            envelope.signature = Some(s.inner.sign_meta(session_id, &envelope.manifest_z));
        }
        let mut meta = envelope.encode();
        let mut flags = 0;
        if !self.recipients.is_empty() {
            meta = crypto::encrypt(&meta, &self.recipients).map_err(js_err)?;
            flags |= FLAG_ENCRYPTED;
        }
        let symbol_size = params.symbol_size();
        if meta.len() as u64 > fec::max_segment_len(symbol_size) {
            return Err(js_err("the file list is too large for this QR density"));
        }
        let layout = SessionLayout {
            session_id,
            flags,
            seg_shift,
            meta_len: meta.len() as u32,
            body_len: body.len() as u64,
            symbol_size,
        };
        let wire = body.len() + meta.len();
        let summary = summary(&manifest);
        let source = MemorySource {
            meta,
            body,
            seg_shift,
        };
        let config = ScheduleConfig {
            redundancy,
            ..ScheduleConfig::default()
        };
        Ok(SendSession {
            sender: Sender::new(layout, source, config, None),
            params,
            session_id,
            summary,
            wire_bytes: wire as f64,
            encrypted: flags != 0,
            frames: 0,
        })
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

/// An endless stream of QR codes for one transfer.
#[wasm_bindgen]
pub struct SendSession {
    sender: Sender<MemorySource>,
    params: QrParams,
    session_id: u32,
    summary: String,
    wire_bytes: f64,
    encrypted: bool,
    frames: u64,
}

#[wasm_bindgen]
impl SendSession {
    /// The next QR code to show.
    #[wasm_bindgen(js_name = nextQr)]
    pub fn next_qr(&mut self) -> JsResult<QrImage> {
        let frame = self.sender.next_frame().map_err(js_err)?;
        self.frames += 1;
        Ok(qr::render(&frame.to_qr_text(), self.params)
            .map_err(js_err)?
            .into())
    }

    /// The next frame as QR text (for tests).
    #[wasm_bindgen(js_name = nextText)]
    pub fn next_text(&mut self) -> JsResult<String> {
        self.frames += 1;
        Ok(self.sender.next_frame().map_err(js_err)?.to_qr_text())
    }

    #[wasm_bindgen(getter)]
    pub fn session(&self) -> String {
        session_hex(self.session_id)
    }

    #[wasm_bindgen(getter)]
    pub fn summary(&self) -> String {
        self.summary.clone()
    }

    #[wasm_bindgen(getter, js_name = wireBytes)]
    pub fn wire_bytes(&self) -> f64 {
        self.wire_bytes
    }

    #[wasm_bindgen(getter)]
    pub fn encrypted(&self) -> bool {
        self.encrypted
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

    #[wasm_bindgen(getter, js_name = symbolSize)]
    pub fn symbol_size(&self) -> usize {
        self.sender.layout().symbol_size
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
    /// Meta arrived and was opened.
    meta: bool,
    /// Segments that failed verification.
    rejected: Vec<u32>,
    error: Option<String>,
}

#[derive(Serialize)]
struct EntryInfo {
    path: String,
    dir: bool,
    size: u64,
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

/// One incoming transfer.
#[wasm_bindgen]
pub struct Receive {
    rx: Receiver,
    me: Option<DeviceIdentity>,
    trusted: Vec<DevicePublic>,
    meta_bytes: Option<Vec<u8>>,
    manifest: Option<Manifest>,
    signer: Option<[u8; 32]>,
    segments: BTreeMap<u32, Vec<u8>>,
    unverified: Vec<u32>,
    completed: Vec<(u32, Vec<u8>)>,
    fatal: Option<String>,
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
            me: None,
            trusted: Vec::new(),
            meta_bytes: None,
            manifest: None,
            signer: None,
            segments: BTreeMap::new(),
            unverified: Vec::new(),
            completed: Vec::new(),
            fatal: None,
        })
    }

    /// The identity used to decrypt (cloned from its secret).
    #[wasm_bindgen(js_name = setIdentity)]
    pub fn set_identity(&mut self, identity: &Identity) -> JsResult<()> {
        self.me = Some(DeviceIdentity::parse(&identity.inner.to_secret_string()).map_err(js_err)?);
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

    /// Restores state saved earlier: session parameters, then segments.
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

    #[wasm_bindgen(js_name = restoreSegment)]
    pub fn restore_segment(&mut self, index: u32, data: &[u8]) -> JsResult<()> {
        self.rx.mark_done(index);
        if index == 0 {
            if let Err(msg) = self.open_meta(data.to_vec()) {
                self.fatal = Some(msg.clone());
                return Err(js_err(msg));
            }
        } else {
            self.segments.insert(index, data.to_vec());
            if self.manifest.is_none() {
                self.unverified.push(index);
            }
        }
        Ok(())
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

    /// Feeds one decoded QR text.
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
                Event::Completed { index: 0, data } => {
                    self.completed.push((0, data.clone()));
                    match self.open_meta(data) {
                        Ok(rejected) => {
                            out.meta = true;
                            out.completed.push(0);
                            out.rejected.extend(rejected);
                        }
                        Err(msg) => {
                            self.fatal = Some(msg.clone());
                            out.error = Some(msg);
                        }
                    }
                }
                Event::Completed { index, data } => {
                    if self.verify(index, &data) {
                        if self.manifest.is_none() {
                            self.unverified.push(index);
                        }
                        self.completed.push((index, data.clone()));
                        self.segments.insert(index, data);
                        out.completed.push(index);
                    } else {
                        self.rx.reset(index);
                        out.rejected.push(index);
                    }
                }
            }
        }
        to_js(&out)
    }

    /// Completed segments since the last call: `[{ index, data }]` (to persist).
    #[wasm_bindgen(js_name = takeCompleted)]
    pub fn take_completed(&mut self) -> Array {
        self.completed
            .drain(..)
            .map(|(index, data)| {
                let o = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&o, &"index".into(), &index.into());
                let _ = js_sys::Reflect::set(&o, &"data".into(), &Uint8Array::from(&data[..]));
                JsValue::from(o)
            })
            .collect()
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
                            size: e.file_size(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            frames: frames as f64,
            useful: useful as f64,
        })
    }

    #[wasm_bindgen(js_name = isComplete)]
    pub fn is_complete(&self) -> bool {
        self.rx.is_complete() && self.manifest.is_some() && self.unverified.is_empty()
    }

    /// Resume code for the segments still missing.
    #[wasm_bindgen(js_name = resumeCode)]
    pub fn resume_code(&self) -> Option<String> {
        let p = self.rx.params()?;
        let missing = self.rx.missing();
        (!missing.is_empty()).then(|| ResumeCode::new(p.session_id, missing).encode())
    }

    /// Fatal problem with this session (e.g. encrypted for another device).
    #[wasm_bindgen(getter)]
    pub fn error(&self) -> Option<String> {
        self.fatal.clone()
    }

    /// Reassembles, verifies and unpacks the transfer:
    /// `{ kind: "text", text }` or `{ kind: "files", entries: [{ path, dir, data, mtime }] }`.
    pub fn extract(&self) -> JsResult<JsValue> {
        let m = self
            .manifest
            .as_ref()
            .ok_or_else(|| js_err("not complete"))?;
        if !self.is_complete() {
            return Err(js_err("not complete"));
        }
        let mut body = Vec::with_capacity(m.body.length as usize);
        for i in 1..=self.rx.params().unwrap().seg_count {
            body.extend_from_slice(&self.segments[&i]);
        }
        if blake3::hash(&body).to_hex().as_str() != m.body.blake3 {
            return Err(js_err("received data does not match the sender's checksum"));
        }
        let payload = if self.rx.params().unwrap().flags & FLAG_ENCRYPTED != 0 {
            let me = self.me.as_ref().ok_or_else(|| js_err("no identity"))?;
            crypto::decrypt(&body, me, usize::MAX >> 1).map_err(js_err)?
        } else {
            body
        };
        let out = js_sys::Object::new();
        match m.kind {
            Kind::Text => {
                let text = qrsend_core::payload::unpack_text(m, &payload[..]).map_err(js_err)?;
                js_sys::Reflect::set(&out, &"kind".into(), &"text".into()).ok();
                js_sys::Reflect::set(&out, &"text".into(), &text.into()).ok();
            }
            Kind::Files => {
                let mut sink = JsSink(Array::new());
                qrsend_core::payload::unpack(m, &payload[..], &mut sink).map_err(js_err)?;
                js_sys::Reflect::set(&out, &"kind".into(), &"files".into()).ok();
                js_sys::Reflect::set(&out, &"entries".into(), &sink.0).ok();
            }
        }
        Ok(out.into())
    }
}

impl Receive {
    fn verify(&self, index: u32, data: &[u8]) -> bool {
        match &self.manifest {
            Some(m) => m
                .body
                .segment_blake3
                .get(index as usize - 1)
                .is_some_and(|h| *h == segment_hash(data)),
            None => true,
        }
    }

    /// Opens meta; returns body segments that failed late verification.
    fn open_meta(&mut self, data: Vec<u8>) -> Result<Vec<u32>, String> {
        let p = *self.rx.params().ok_or("no session")?;
        let opened = crypto::open_meta(
            &data,
            p.session_id,
            p.flags & FLAG_ENCRYPTED != 0,
            self.me.as_ref(),
        )
        .map_err(|e| match e {
            OpenMetaError::NoIdentity => {
                "This transfer is encrypted. Create your device ID first.".to_string()
            }
            OpenMetaError::NotForUs => "This transfer is encrypted for another device.".to_string(),
            e => e.to_string(),
        })?;
        let m = opened.manifest;
        if m.body.length.div_ceil(1 << p.seg_shift) != p.seg_count as u64
            || m.body.segment_blake3.len() != p.seg_count as usize
        {
            return Err("manifest does not match the transfer layout".into());
        }
        self.manifest = Some(m);
        self.signer = opened.signer;
        self.meta_bytes = Some(data);
        let mut rejected = Vec::new();
        for index in std::mem::take(&mut self.unverified) {
            let ok = self
                .segments
                .get(&index)
                .is_some_and(|d| self.verify(index, d));
            if !ok {
                self.segments.remove(&index);
                self.rx.reset(index);
                rejected.push(index);
            }
        }
        Ok(rejected)
    }
}

struct JsSink(Array);

impl UnpackSink for JsSink {
    fn dir(&mut self, path: &SafePath, _: &Entry) -> std::io::Result<()> {
        let o = js_sys::Object::new();
        js_sys::Reflect::set(&o, &"path".into(), &path.to_string().into()).ok();
        js_sys::Reflect::set(&o, &"dir".into(), &true.into()).ok();
        self.0.push(&o);
        Ok(())
    }

    fn file(
        &mut self,
        path: &SafePath,
        entry: &Entry,
        content: &mut dyn std::io::Read,
    ) -> std::io::Result<()> {
        let mut data = Vec::with_capacity(entry.file_size() as usize);
        content.read_to_end(&mut data)?;
        let o = js_sys::Object::new();
        js_sys::Reflect::set(&o, &"path".into(), &path.to_string().into()).ok();
        js_sys::Reflect::set(&o, &"dir".into(), &false.into()).ok();
        js_sys::Reflect::set(&o, &"data".into(), &Uint8Array::from(&data[..])).ok();
        if let Some(mtime) = entry.mtime {
            js_sys::Reflect::set(&o, &"mtime".into(), &((mtime as f64) * 1000.0).into()).ok();
        }
        self.0.push(&o);
        Ok(())
    }
}
