//! WebAssembly core of the QRSend library for web pages: files, folders or a
//! text become an endless stream of QR codes ([`Encoder`]), and the texts a
//! camera reads off them become the files again ([`Decoder`]).
//!
//! Only unencrypted transfers: nothing here holds keys. Everything is kept in
//! WebAssembly memory, which suits what QR codes carry (a camera reads tens of
//! kilobytes a second). The streams are those of the QRSend app and CLI
//! (`docs/PROTOCOL.md`), so each side can be either of them.
//!
//! The JavaScript side (`packages/qrsend-web`) wraps these in friendlier
//! classes; this API is not meant to be used directly.
//!
//! QR Code is a registered trademark of DENSO WAVE INCORPORATED in Japan and
//! in other countries.

use std::io::{self, Read};

use js_sys::{Function, Object, Reflect, Uint8Array};
use qrsend_core::fec;
use qrsend_core::frame::{FLAG_ENCRYPTED, Frame, META_INDEX};
use qrsend_core::manifest::{
    Body, Entry, EntryType, Kind, MANIFEST_VERSION, Manifest, MetaEnvelope, content_session_id,
    session_hex,
};
use qrsend_core::payload::{BodyHasher, PackOptions, Packed, Packer, UnpackSink, segment_hash};
use qrsend_core::qr::{self, Ec, QrParams};
use qrsend_core::receiver::{Event, Receiver};
use qrsend_core::sanitize::SafePath;
use qrsend_core::schedule::ScheduleConfig;
use qrsend_core::sender::{MemorySource, Sender, SessionLayout};
use serde::Serialize;
use wasm_bindgen::prelude::*;

type JsResult<T> = Result<T, JsError>;

/// Segments of 1 MiB, as the app and the CLI send them.
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

/// Seconds since 1970, from milliseconds as in `File.lastModified`.
fn seconds(ms: Option<f64>) -> Option<i64> {
    ms.filter(|ms| ms.is_finite())
        .map(|ms| (ms / 1000.0) as i64)
}

#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

// ---------------------------------------------------------------- sending

/// A receiver refuses a transfer with a path that could land outside the
/// folder it saves to (`..`, absolute paths): refused here instead, before
/// anything is sent.
fn receivable(path: &str) -> JsResult<()> {
    SafePath::parse(path)
        .map(drop)
        .map_err(|e| js_err(format!("unsafe path: {e}")))
}

/// The packed body, kept in memory, with its checksums made on the way.
type PackedBody = BodyHasher<Vec<u8>>;

struct Stream {
    sender: Sender<MemorySource>,
    params: QrParams,
    session_id: u32,
    bytes: u64,
    count: u64,
}

/// Packs files, folders or a text and then hands out the codes to show, one
/// after another, for as long as they are asked for.
///
/// Usage: `addDir` / `beginFile` + `write` + `endFile` (or `setText`), then
/// `finish()`, then `next()` or `nextModules()` again and again.
#[wasm_bindgen]
pub struct Encoder {
    pack: PackOptions,
    qr: Option<QrParams>,
    redundancy: f64,
    packer: Option<Packer<PackedBody>>,
    text: Option<String>,
    stream: Option<Stream>,
}

#[wasm_bindgen]
impl Encoder {
    /// `qr_version` 0 picks the code size from the size of the transfer;
    /// otherwise 1–40 with `ec` one of L, M, Q, H. `compress` false sends
    /// the data as it is (for data that is compressed already).
    #[wasm_bindgen(constructor)]
    pub fn new(qr_version: u8, ec: &str, compress: bool, redundancy: f64) -> JsResult<Encoder> {
        let qr = match qr_version {
            0 => None,
            1..=40 => Some(QrParams {
                version: qr_version,
                ec: ec.parse::<Ec>().map_err(js_err)?,
            }),
            _ => return Err(js_err("the QR version must be 1 to 40 (or 0 to choose)")),
        };
        if !(0.0..=10.0).contains(&redundancy) {
            return Err(js_err("redundancy must be 0 to 10"));
        }
        Ok(Encoder {
            pack: PackOptions {
                zstd_level: compress.then_some(3),
                zstd_workers: 0,
            },
            qr,
            redundancy,
            packer: None,
            text: None,
            stream: None,
        })
    }

    fn packer(&mut self) -> JsResult<&mut Packer<PackedBody>> {
        if self.stream.is_some() {
            return Err(js_err("finish() was called already"));
        }
        if self.text.is_some() {
            return Err(js_err("a transfer is either a text or files"));
        }
        if self.packer.is_none() {
            let body = BodyHasher::new(Vec::new(), SEG_SHIFT);
            self.packer = Some(Packer::new(body, self.pack).map_err(js_err)?);
        }
        Ok(self.packer.as_mut().unwrap())
    }

    /// A folder (also an empty one). `last_modified` in milliseconds.
    #[wasm_bindgen(js_name = addDir)]
    pub fn add_dir(&mut self, path: &str, last_modified: Option<f64>) -> JsResult<()> {
        receivable(path)?;
        self.packer()?.add_dir(path, seconds(last_modified));
        Ok(())
    }

    /// Starts a file: its content follows with `write`, then `endFile`.
    /// `path` uses `/` between folders.
    #[wasm_bindgen(js_name = beginFile)]
    pub fn begin_file(&mut self, path: &str, last_modified: Option<f64>) -> JsResult<()> {
        receivable(path)?;
        self.packer()?
            .begin_file(path, None, seconds(last_modified));
        Ok(())
    }

    pub fn write(&mut self, chunk: &[u8]) -> JsResult<()> {
        self.packer()?.write_chunk(chunk).map_err(js_err)
    }

    #[wasm_bindgen(js_name = endFile)]
    pub fn end_file(&mut self) -> JsResult<()> {
        self.packer()?.end_file(None).map_err(js_err)?;
        Ok(())
    }

    /// Sends a text instead of files.
    #[wasm_bindgen(js_name = setText)]
    pub fn set_text(&mut self, text: &str) -> JsResult<()> {
        if self.stream.is_some() {
            return Err(js_err("finish() was called already"));
        }
        if self.packer.is_some() {
            return Err(js_err("a transfer is either a text or files"));
        }
        self.text = Some(text.to_string());
        Ok(())
    }

    /// Ends the content. The codes can be had from now on.
    pub fn finish(&mut self) -> JsResult<()> {
        if self.stream.is_some() {
            return Err(js_err("finish() was called already"));
        }
        let (hasher, packed): (PackedBody, Packed) = match self.text.take() {
            Some(text) => {
                let body = BodyHasher::new(Vec::new(), SEG_SHIFT);
                Packer::text(body, &text, self.pack).map_err(js_err)?
            }
            None => {
                self.packer()?;
                self.packer.take().unwrap().finish().map_err(js_err)?
            }
        };
        let (body, digest) = hasher.finish().map_err(js_err)?;
        // The session id follows from the content (and so nothing in the
        // manifest may depend on when it is made): sending the same data
        // again continues a receiver that stopped halfway (PROTOCOL §8.1).
        let newest = packed.entries.iter().filter_map(|e| e.mtime).max();
        let mut manifest = Manifest {
            qrsend: MANIFEST_VERSION,
            session: String::new(),
            kind: packed.kind,
            created: newest.unwrap_or(0),
            sender_name: None,
            body: Body {
                length: digest.length,
                blake3: digest.blake3,
                segment_blake3: digest.segment_blake3,
                encoding: packed.encoding,
                plain_length: packed.plain_length,
            },
            entries: packed.entries,
        };
        let session_id = content_session_id(&manifest);
        manifest.session = session_hex(session_id);
        let meta = MetaEnvelope::from_manifest(&manifest)
            .map_err(js_err)?
            .encode();
        let params = self
            .qr
            .unwrap_or_else(|| qr::auto_params(meta.len() as u32, body.len() as u64));
        let symbol_size = params.symbol_size();
        if meta.len() as u64 > fec::max_segment_len(symbol_size) {
            return Err(js_err(
                "the list of files is too long for codes this small: choose a larger QR version",
            ));
        }
        let layout = SessionLayout {
            session_id,
            flags: 0,
            seg_shift: SEG_SHIFT,
            meta_len: meta.len() as u32,
            body_len: body.len() as u64,
            symbol_size,
        };
        let bytes = meta.len() as u64 + body.len() as u64;
        let config = ScheduleConfig {
            redundancy: self.redundancy,
            ..ScheduleConfig::default()
        };
        let source = MemorySource {
            meta,
            body,
            seg_shift: SEG_SHIFT,
        };
        self.stream = Some(Stream {
            sender: Sender::new(layout, source, config, None),
            params,
            session_id,
            bytes,
            count: 0,
        });
        Ok(())
    }

    fn stream(&mut self) -> JsResult<&mut Stream> {
        self.stream
            .as_mut()
            .ok_or_else(|| js_err("call finish() first"))
    }

    /// The text of the next code (QR alphanumeric mode).
    #[wasm_bindgen(js_name = next)]
    pub fn next_text(&mut self) -> JsResult<String> {
        let stream = self.stream()?;
        stream.count += 1;
        Ok(stream.sender.next_frame().map_err(js_err)?.to_qr_text())
    }

    /// The next code drawn: `size × size` modules, row by row, 1 = dark,
    /// without the quiet zone.
    #[wasm_bindgen(js_name = nextModules)]
    pub fn next_modules(&mut self) -> JsResult<Uint8Array> {
        let text = self.next_text()?;
        let params = self.stream()?.params;
        let m = qr::render(&text, params).map_err(js_err)?;
        let modules: Vec<u8> = m.modules.into_iter().map(u8::from).collect();
        Ok(Uint8Array::from(&modules[..]))
    }

    /// Modules on a side of every code.
    #[wasm_bindgen(getter)]
    pub fn size(&self) -> JsResult<usize> {
        Ok(self.sealed()?.params.modules())
    }

    #[wasm_bindgen(getter, js_name = qrVersion)]
    pub fn qr_version(&self) -> JsResult<u8> {
        Ok(self.sealed()?.params.version)
    }

    #[wasm_bindgen(getter, js_name = errorCorrection)]
    pub fn error_correction(&self) -> JsResult<String> {
        Ok(format!("{:?}", self.sealed()?.params.ec))
    }

    /// Codes in one round: everything once, with some to spare.
    #[wasm_bindgen(getter, js_name = codesPerRound)]
    pub fn codes_per_round(&self) -> JsResult<f64> {
        Ok(self.sealed()?.sender.frames_per_pass() as f64)
    }

    /// Codes handed out so far.
    #[wasm_bindgen(getter)]
    pub fn count(&self) -> JsResult<f64> {
        Ok(self.sealed()?.count as f64)
    }

    /// Bytes the codes carry (after compression).
    #[wasm_bindgen(getter)]
    pub fn bytes(&self) -> JsResult<f64> {
        Ok(self.sealed()?.bytes as f64)
    }

    /// The transfer's id (8 hex digits): the same for the same data.
    #[wasm_bindgen(getter)]
    pub fn session(&self) -> JsResult<String> {
        Ok(session_hex(self.sealed()?.session_id))
    }

    fn sealed(&self) -> JsResult<&Stream> {
        self.stream
            .as_ref()
            .ok_or_else(|| js_err("call finish() first"))
    }
}

// ---------------------------------------------------------------- receiving

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProgressInfo {
    /// "waiting" (no code of a transfer yet), "receiving", "complete".
    state: &'static str,
    session: Option<String>,
    /// Codes of the transfer read, repeats included.
    codes: f64,
    /// Of those, the ones that brought something new.
    useful_codes: f64,
    /// About how many more codes it takes (a lower bound).
    remaining_codes: Option<f64>,
    received_bytes: Option<f64>,
    total_bytes: Option<f64>,
    /// 0 to 1.
    ratio: f64,
    /// Once the list of files has arrived: "files" or "text".
    kind: Option<&'static str>,
    file_count: Option<usize>,
    /// The size of the files (or the text) once received.
    size: Option<f64>,
    /// Codes of an encrypted transfer were read (and passed over).
    encrypted: bool,
    /// Codes of another transfer were read (and passed over).
    other_transfer: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EntryInfo {
    path: String,
    directory: bool,
    size: f64,
    last_modified: Option<f64>,
}

/// Takes the texts read off the codes, in any order, with repeats and gaps,
/// and puts the transfer together. It follows the first transfer whose
/// codes it reads; codes of others are passed over.
#[wasm_bindgen]
pub struct Decoder {
    rx: Receiver,
    manifest: Option<Manifest>,
    /// Body segments (index 1 at 0). Verified once the manifest is known.
    segments: Vec<Option<Vec<u8>>>,
    encrypted: bool,
    other: bool,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl Decoder {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Decoder {
        Decoder {
            rx: Receiver::new(),
            manifest: None,
            segments: Vec::new(),
            encrypted: false,
            other: false,
        }
    }

    /// Takes the text of one code. Returns whether anything changed that
    /// `progress()` tells (most repeats change nothing). Texts that are not
    /// QRSend codes are passed over.
    pub fn push(&mut self, text: &str) -> bool {
        let Ok(frame) = Frame::from_qr_text(text.trim()) else {
            return false;
        };
        if frame.header.flags & FLAG_ENCRYPTED != 0 && self.rx.params().is_none() {
            return !std::mem::replace(&mut self.encrypted, true);
        }
        let useful = self.rx.stats().1;
        let mut changed = false;
        for event in self.rx.push(frame) {
            changed = true;
            match event {
                Event::Locked(p) => self.segments = vec![None; p.seg_count as usize],
                Event::ForeignSession(_) => {
                    changed = !std::mem::replace(&mut self.other, true);
                }
                Event::Inconsistent => changed = false,
                Event::Completed { index, data } => self.completed(index, data),
            }
        }
        changed || self.rx.stats().1 > useful
    }

    fn completed(&mut self, index: u32, data: Vec<u8>) {
        if index == META_INDEX {
            match self.open_meta(&data) {
                Some(m) => {
                    // What came before the list of files is checked now.
                    for (i, slot) in self.segments.iter_mut().enumerate() {
                        let fits = |data: &Vec<u8>| {
                            m.body.segment_blake3.get(i) == Some(&segment_hash(data))
                        };
                        if slot.as_ref().is_some_and(|data| !fits(data)) {
                            *slot = None;
                            self.rx.reset(i as u32 + 1);
                        }
                    }
                    self.manifest = Some(m);
                }
                // Damaged: collected anew.
                None => self.rx.reset(META_INDEX),
            }
            return;
        }
        let i = index as usize - 1;
        let fits = match &self.manifest {
            Some(m) => m.body.segment_blake3.get(i) == Some(&segment_hash(&data)),
            None => true,
        };
        match self.segments.get_mut(i) {
            Some(slot) if fits => *slot = Some(data),
            _ => self.rx.reset(index),
        }
    }

    fn open_meta(&self, data: &[u8]) -> Option<Manifest> {
        let p = *self.rx.params()?;
        // A signature (from a QRSend device) is not checked here: these
        // transfers are unencrypted, and the sender is not known to us.
        let m = MetaEnvelope::decode(data)
            .ok()?
            .manifest(p.session_id)
            .ok()?;
        let fits = m.body.length.div_ceil(1 << p.seg_shift) == p.seg_count as u64
            && m.body.segment_blake3.len() == p.seg_count as usize;
        fits.then_some(m)
    }

    #[wasm_bindgen(js_name = isComplete)]
    pub fn is_complete(&self) -> bool {
        self.manifest.is_some()
            && self.rx.is_complete()
            && self.segments.iter().all(Option::is_some)
    }

    /// `{ state, session, codes, usefulCodes, remainingCodes, receivedBytes,
    /// totalBytes, ratio, kind, fileCount, size, encrypted, otherTransfer }`
    pub fn progress(&self) -> JsResult<JsValue> {
        let p = self.rx.params();
        let m = self.manifest.as_ref();
        let (codes, useful, _) = self.rx.stats();
        let progress = self.rx.progress();
        let complete = self.is_complete();
        let ratio = match progress {
            _ if complete => 1.0,
            Some(pr) if pr.total_bytes > 0 => {
                (pr.total_bytes - pr.remaining_bytes) as f64 / pr.total_bytes as f64
            }
            _ => 0.0,
        };
        to_js(&ProgressInfo {
            state: match (p, complete) {
                (None, _) => "waiting",
                (Some(_), false) => "receiving",
                (Some(_), true) => "complete",
            },
            session: p.map(|p| session_hex(p.session_id)),
            codes: codes as f64,
            useful_codes: useful as f64,
            remaining_codes: progress.map(|p| p.remaining_symbols as f64),
            received_bytes: progress.map(|p| (p.total_bytes - p.remaining_bytes) as f64),
            total_bytes: progress.map(|p| p.total_bytes as f64),
            ratio,
            kind: m.map(|m| match m.kind {
                Kind::Text => "text",
                Kind::Files => "files",
            }),
            file_count: m.map(Manifest::file_count),
            size: m.map(|m| m.body.plain_length as f64),
            encrypted: self.encrypted,
            other_transfer: self.other,
        })
    }

    /// What the transfer holds, as the sender named it, once the list has
    /// arrived (empty before): `[{ path, directory, size, lastModified }]`.
    /// The paths of the result can differ (made safe to save).
    pub fn entries(&self) -> JsResult<JsValue> {
        let entries: Vec<EntryInfo> = self
            .manifest
            .iter()
            .flat_map(|m| &m.entries)
            .map(|e| EntryInfo {
                path: e.path.clone(),
                directory: e.kind == EntryType::Dir,
                size: e.file_size() as f64,
                last_modified: e.mtime.map(|t| t as f64 * 1000.0),
            })
            .collect();
        to_js(&entries)
    }

    /// Checks the whole transfer and hands it out: `sink.text(string)` for a
    /// text, or for files `sink.dir(path, lastModified)`, and `sink.begin(path,
    /// size, lastModified)`, `sink.write(Uint8Array)`…, `sink.end()` for each
    /// file. Paths are made safe to save (no `..`, no absolute paths, no
    /// names Windows refuses, no two the same). Returns "text" or "files".
    pub fn extract(&self, sink: Object) -> JsResult<String> {
        let m = self
            .manifest
            .as_ref()
            .filter(|_| self.is_complete())
            .ok_or_else(|| js_err("the transfer is not complete"))?;
        let mut hasher = blake3::Hasher::new();
        io::copy(&mut Segments::new(&self.segments), &mut hasher).map_err(js_err)?;
        if hasher.finalize().to_hex().as_str() != m.body.blake3 {
            return Err(js_err(
                "the received data does not match the sender's checksum",
            ));
        }
        let body = Segments::new(&self.segments);
        match m.kind {
            Kind::Text => {
                let text = qrsend_core::payload::unpack_text(m, body).map_err(js_err)?;
                call(&sink, "text", &[text.into()]).map_err(js_err)?;
                Ok("text".into())
            }
            Kind::Files => {
                qrsend_core::payload::unpack(m, body, &mut JsSink(sink)).map_err(js_err)?;
                Ok("files".into())
            }
        }
    }
}

/// The body segments read one after another.
struct Segments<'a> {
    rest: std::slice::Iter<'a, Option<Vec<u8>>>,
    current: &'a [u8],
}

impl<'a> Segments<'a> {
    fn new(segments: &'a [Option<Vec<u8>>]) -> Self {
        Segments {
            rest: segments.iter(),
            current: &[],
        }
    }
}

impl Read for Segments<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.current.is_empty() {
            match self.rest.next() {
                None => return Ok(0),
                Some(segment) => {
                    self.current = segment
                        .as_deref()
                        .ok_or_else(|| io::Error::other("a segment is missing"))?;
                }
            }
        }
        self.current.read(buf)
    }
}

fn call(target: &Object, method: &str, args: &[JsValue]) -> io::Result<()> {
    let f: Function = Reflect::get(target, &method.into()).map_err(io_err)?.into();
    let args: js_sys::Array = args.iter().collect();
    f.apply(target, &args).map_err(io_err)?;
    Ok(())
}

fn millis(entry: &Entry) -> JsValue {
    entry
        .mtime
        .map(|t| JsValue::from(t as f64 * 1000.0))
        .unwrap_or(JsValue::UNDEFINED)
}

struct JsSink(Object);

impl UnpackSink for JsSink {
    fn dir(&mut self, path: &SafePath, entry: &Entry) -> io::Result<()> {
        call(&self.0, "dir", &[path.to_string().into(), millis(entry)])
    }

    fn file(&mut self, path: &SafePath, entry: &Entry, content: &mut dyn Read) -> io::Result<()> {
        call(
            &self.0,
            "begin",
            &[
                path.to_string().into(),
                (entry.file_size() as f64).into(),
                millis(entry),
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

#[cfg(test)]
mod tests {
    use super::*;

    // Natively, anything that makes a `JsValue` or a `JsError` panics, so
    // these keep to what does not. The rest is tested from JavaScript
    // (packages/qrsend-web).

    fn encoder() -> Encoder {
        Encoder::new(0, "M", true, 0.1).unwrap_or_else(|_| unreachable!())
    }

    fn texts(e: &mut Encoder, n: usize) -> Vec<String> {
        (0..n)
            .map(|_| e.next_text().unwrap_or_else(|_| unreachable!()))
            .collect()
    }

    fn ok<T>(r: JsResult<T>) -> T {
        r.unwrap_or_else(|_| panic!("a JsError"))
    }

    fn files(data: &[u8]) -> Encoder {
        let mut e = encoder();
        ok(e.add_dir("photos", Some(1_700_000_000_000.0)));
        ok(e.begin_file("photos/a.bin", Some(1_700_000_000_000.0)));
        for chunk in data.chunks(1000) {
            ok(e.write(chunk));
        }
        ok(e.end_file());
        ok(e.begin_file("note.txt", None));
        ok(e.write(b"hello"));
        ok(e.end_file());
        ok(e.finish());
        e
    }

    fn noise(n: usize) -> Vec<u8> {
        let mut x = 0x1234_5678u32;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect()
    }

    #[test]
    fn files_come_through_with_codes_lost_and_out_of_order() {
        let data = noise(300_000);
        let mut e = files(&data);
        let per_round = ok(e.codes_per_round()) as usize;
        let mut codes = texts(&mut e, per_round * 2);
        // Every third code is lost, the rest arrive back to front.
        codes = codes
            .into_iter()
            .enumerate()
            .filter(|(i, _)| i % 3 != 0)
            .map(|(_, c)| c)
            .collect();
        codes.reverse();
        let mut d = Decoder::new();
        for c in &codes {
            d.push(c);
            if d.is_complete() {
                break;
            }
        }
        assert!(d.is_complete());
        let m = d.manifest.as_ref().unwrap();
        assert_eq!(m.kind, Kind::Files);
        assert_eq!(m.file_count(), 2);
        let mut body = Vec::new();
        Segments::new(&d.segments).read_to_end(&mut body).unwrap();
        assert_eq!(blake3::hash(&body).to_hex().as_str(), m.body.blake3);
    }

    #[test]
    fn the_same_data_is_the_same_transfer() {
        let data = noise(10_000);
        let a = files(&data);
        let b = files(&data);
        assert_eq!(ok(a.session()), ok(b.session()));
        assert_ne!(ok(a.session()), ok(files(&noise(9_999)).session()));
    }

    #[test]
    fn codes_fit_the_version_asked_for() {
        let mut e = Encoder::new(20, "L", true, 0.1).unwrap_or_else(|_| unreachable!());
        ok(e.set_text(&"x".repeat(5000)));
        ok(e.finish());
        assert_eq!(ok(e.size()), 97);
        let params = QrParams {
            version: 20,
            ec: Ec::L,
        };
        for text in texts(&mut e, 10) {
            assert_eq!(qr::render(&text, params).unwrap().width, 97);
        }
    }

    #[test]
    fn other_texts_are_passed_over() {
        let mut d = Decoder::new();
        assert!(!d.push("https://example.com"));
        assert!(!d.push("QSC1-NOTHING"));
        assert!(d.rx.params().is_none());
    }
}
