//! Body payload packing / unpacking (docs/PROTOCOL.md §6).

use std::collections::HashSet;
use std::io::{self, BufReader, Read, Write};

use crate::compress;
use crate::manifest::{Encoding, Entry, EntryType, Kind, Manifest};
use crate::sanitize::{PathError, SafePath, numbered_name};

enum Sink<W: Write> {
    Zstd(zstd::stream::write::Encoder<'static, W>),
    Raw(W),
}

impl<W: Write> Write for Sink<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Sink::Zstd(w) => w.write(buf),
            Sink::Raw(w) => w.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Sink::Zstd(w) => w.flush(),
            Sink::Raw(w) => w.flush(),
        }
    }
}

/// Packing options.
#[derive(Debug, Clone, Copy)]
pub struct PackOptions {
    /// `None` disables compression.
    pub zstd_level: Option<i32>,
    pub zstd_workers: u32,
}

impl Default for PackOptions {
    fn default() -> Self {
        PackOptions {
            zstd_level: Some(19),
            zstd_workers: 0,
        }
    }
}

/// Streams entries into the (compressed) payload.
pub struct Packer<W: Write> {
    sink: Sink<W>,
    encoding: Encoding,
    entries: Vec<Entry>,
    plain_length: u64,
}

/// Result of packing: everything the manifest needs about the payload.
#[derive(Debug, Clone)]
pub struct Packed {
    pub kind: Kind,
    pub encoding: Encoding,
    pub entries: Vec<Entry>,
    pub plain_length: u64,
}

impl<W: Write> Packer<W> {
    pub fn new(inner: W, opts: PackOptions) -> io::Result<Self> {
        let (sink, encoding) = match opts.zstd_level {
            Some(level) => (
                Sink::Zstd(compress::encoder(inner, level, opts.zstd_workers)?),
                Encoding::Zstd,
            ),
            None => (Sink::Raw(inner), Encoding::None),
        };
        Ok(Packer {
            sink,
            encoding,
            entries: Vec::new(),
            plain_length: 0,
        })
    }

    pub fn add_dir(&mut self, path: &str, mtime: Option<i64>) {
        self.entries.push(Entry {
            path: path.to_string(),
            kind: EntryType::Dir,
            size: None,
            blake3: None,
            mode: None,
            mtime,
        });
    }

    /// Copies a file's content; fails if `expected_size` is given and differs
    /// (the file changed while being read).
    pub fn add_file(
        &mut self,
        path: &str,
        reader: &mut dyn Read,
        expected_size: Option<u64>,
        mode: Option<u32>,
        mtime: Option<i64>,
    ) -> io::Result<u64> {
        let mut hasher = blake3::Hasher::new();
        let mut buf = vec![0u8; 256 * 1024];
        let mut size = 0u64;
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            hasher.update(&buf[..n]);
            self.sink.write_all(&buf[..n])?;
            size += n as u64;
        }
        if expected_size.is_some_and(|e| e != size) {
            return Err(io::Error::other(format!(
                "{path}: file size changed while reading"
            )));
        }
        self.plain_length += size;
        self.entries.push(Entry {
            path: path.to_string(),
            kind: EntryType::File,
            size: Some(size),
            blake3: Some(hasher.finalize().to_hex().to_string()),
            mode,
            mtime,
        });
        Ok(size)
    }

    fn close(self) -> io::Result<(W, Encoding, Vec<Entry>, u64)> {
        let inner = match self.sink {
            Sink::Zstd(enc) => enc.finish()?,
            Sink::Raw(mut w) => {
                w.flush()?;
                w
            }
        };
        Ok((inner, self.encoding, self.entries, self.plain_length))
    }

    pub fn finish(self) -> io::Result<(W, Packed)> {
        let (w, encoding, entries, plain_length) = self.close()?;
        Ok((
            w,
            Packed {
                kind: Kind::Files,
                encoding,
                entries,
                plain_length,
            },
        ))
    }

    /// Packs a text transfer (no entries).
    pub fn text(inner: W, text: &str, opts: PackOptions) -> io::Result<(W, Packed)> {
        let mut p = Packer::new(inner, opts)?;
        p.sink.write_all(text.as_bytes())?;
        let (w, encoding, _, _) = p.close()?;
        Ok((
            w,
            Packed {
                kind: Kind::Text,
                encoding,
                entries: Vec::new(),
                plain_length: text.len() as u64,
            },
        ))
    }
}

/// Digest of the transmitted body: whole-body and per-segment BLAKE3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyDigest {
    pub length: u64,
    pub blake3: String,
    pub segment_blake3: Vec<String>,
}

/// Write adapter that hashes the transmitted body as it is written.
pub struct BodyHasher<W> {
    inner: W,
    seg_size: u64,
    total: blake3::Hasher,
    seg: blake3::Hasher,
    seg_fill: u64,
    length: u64,
    segments: Vec<String>,
}

impl<W: Write> BodyHasher<W> {
    pub fn new(inner: W, seg_shift: u8) -> Self {
        BodyHasher {
            inner,
            seg_size: 1 << seg_shift,
            total: blake3::Hasher::new(),
            seg: blake3::Hasher::new(),
            seg_fill: 0,
            length: 0,
            segments: Vec::new(),
        }
    }

    pub fn finish(mut self) -> io::Result<(W, BodyDigest)> {
        self.inner.flush()?;
        if self.seg_fill > 0 {
            self.segments.push(self.seg.finalize().to_hex().to_string());
        }
        let digest = BodyDigest {
            length: self.length,
            blake3: self.total.finalize().to_hex().to_string(),
            segment_blake3: self.segments,
        };
        Ok((self.inner, digest))
    }
}

impl<W: Write> Write for BodyHasher<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        let mut rest = &buf[..n];
        self.total.update(rest);
        self.length += n as u64;
        while !rest.is_empty() {
            let take = ((self.seg_size - self.seg_fill) as usize).min(rest.len());
            self.seg.update(&rest[..take]);
            self.seg_fill += take as u64;
            rest = &rest[take..];
            if self.seg_fill == self.seg_size {
                self.segments.push(self.seg.finalize().to_hex().to_string());
                self.seg = blake3::Hasher::new();
                self.seg_fill = 0;
            }
        }
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

pub fn segment_hash(data: &[u8]) -> String {
    blake3::hash(data).to_hex().to_string()
}

#[derive(Debug, thiserror::Error)]
pub enum UnpackError {
    #[error("unsafe path: {0}")]
    Path(#[from] PathError),
    #[error("duplicate path {0}")]
    Duplicate(String),
    #[error("{0}: content does not match its hash")]
    HashMismatch(String),
    #[error("payload is shorter than the manifest declares")]
    Truncated,
    #[error("payload is longer than the manifest declares")]
    TrailingData,
    #[error("text is not valid UTF-8")]
    NotUtf8,
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Where unpacked entries go.
pub trait UnpackSink {
    fn dir(&mut self, path: &SafePath, entry: &Entry) -> io::Result<()>;
    /// Must consume `content` (exactly `entry.size` bytes).
    fn file(&mut self, path: &SafePath, entry: &Entry, content: &mut dyn Read) -> io::Result<()>;
}

/// Maps manifest entries to safe, collision-free paths (in manifest order).
pub fn plan_paths(manifest: &Manifest) -> Result<Vec<SafePath>, UnpackError> {
    let mut exact = HashSet::new();
    let mut folded = HashSet::new();
    let mut out = Vec::with_capacity(manifest.entries.len());
    for e in &manifest.entries {
        let mut p = SafePath::parse(&e.path)?;
        if !exact.insert(p.clone()) {
            return Err(UnpackError::Duplicate(e.path.clone()));
        }
        // Case-insensitive file systems would silently merge these; rename files.
        if e.kind == EntryType::File {
            let base = p.file_name().to_string();
            let mut n = 1;
            while folded.contains(&p.fold_key()) {
                p = p.with_file_name(numbered_name(&base, n));
                n += 1;
            }
        }
        folded.insert(p.fold_key());
        out.push(p);
    }
    Ok(out)
}

fn decoded_reader<'a, R: Read + 'a>(encoding: Encoding, body: R) -> io::Result<Box<dyn Read + 'a>> {
    Ok(match encoding {
        Encoding::Zstd => Box::new(compress::decoder(BufReader::new(body))?),
        Encoding::None => Box::new(body),
    })
}

struct HashingReader<'a> {
    inner: &'a mut dyn Read,
    hasher: blake3::Hasher,
    remaining: u64,
}

impl Read for HashingReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return Ok(0);
        }
        let max = (buf.len() as u64).min(self.remaining) as usize;
        let n = self.inner.read(&mut buf[..max])?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "payload truncated",
            ));
        }
        self.hasher.update(&buf[..n]);
        self.remaining -= n as u64;
        Ok(n)
    }
}

/// Unpacks a `files` payload from the (decrypted) body into `sink`,
/// verifying every file hash and the exact payload length.
pub fn unpack<R: Read>(
    manifest: &Manifest,
    body: R,
    sink: &mut dyn UnpackSink,
) -> Result<(), UnpackError> {
    let paths = plan_paths(manifest)?;
    let mut reader = decoded_reader(manifest.body.encoding, body)?;
    for (entry, path) in manifest.entries.iter().zip(&paths) {
        match entry.kind {
            EntryType::Dir => sink.dir(path, entry)?,
            EntryType::File => {
                let mut hr = HashingReader {
                    inner: &mut *reader,
                    hasher: blake3::Hasher::new(),
                    remaining: entry.file_size(),
                };
                match sink.file(path, entry, &mut hr) {
                    Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                        return Err(UnpackError::Truncated);
                    }
                    r => r?,
                }
                io::copy(&mut hr, &mut io::sink()).map_err(|_| UnpackError::Truncated)?;
                let actual = hr.hasher.finalize().to_hex().to_string();
                if Some(actual.as_str()) != entry.blake3.as_deref() {
                    return Err(UnpackError::HashMismatch(path.to_string()));
                }
            }
        }
    }
    let mut probe = [0u8; 1];
    if reader.read(&mut probe)? != 0 {
        return Err(UnpackError::TrailingData);
    }
    Ok(())
}

/// Reads a `text` payload.
pub fn unpack_text<R: Read>(manifest: &Manifest, body: R) -> Result<String, UnpackError> {
    let reader = decoded_reader(manifest.body.encoding, body)?;
    let mut buf = Vec::new();
    reader
        .take(manifest.body.plain_length + 1)
        .read_to_end(&mut buf)?;
    match (buf.len() as u64).cmp(&manifest.body.plain_length) {
        std::cmp::Ordering::Less => Err(UnpackError::Truncated),
        std::cmp::Ordering::Greater => Err(UnpackError::TrailingData),
        std::cmp::Ordering::Equal => String::from_utf8(buf).map_err(|_| UnpackError::NotUtf8),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Body, session_hex};
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct MemSink(BTreeMap<String, Vec<u8>>);

    impl UnpackSink for MemSink {
        fn dir(&mut self, path: &SafePath, _: &Entry) -> io::Result<()> {
            self.0.insert(format!("{path}/"), Vec::new());
            Ok(())
        }
        fn file(&mut self, path: &SafePath, _: &Entry, content: &mut dyn Read) -> io::Result<()> {
            let mut v = Vec::new();
            content.read_to_end(&mut v)?;
            self.0.insert(path.to_string(), v);
            Ok(())
        }
    }

    fn manifest(packed: Packed) -> Manifest {
        Manifest {
            qrsend: 1,
            session: session_hex(1),
            kind: packed.kind,
            created: 0,
            sender_name: None,
            body: Body {
                length: 0,
                blake3: String::new(),
                segment_blake3: vec![],
                encoding: packed.encoding,
                plain_length: packed.plain_length,
            },
            entries: packed.entries,
        }
    }

    fn pack(opts: PackOptions) -> (Vec<u8>, Manifest) {
        let mut p = Packer::new(Vec::new(), opts).unwrap();
        p.add_dir("d", None);
        p.add_file("d/a.txt", &mut &b"hello"[..], Some(5), None, None)
            .unwrap();
        p.add_file("d/A.TXT", &mut &b"other"[..], None, None, None)
            .unwrap();
        p.add_file("empty", &mut &b""[..], Some(0), None, None)
            .unwrap();
        let (body, packed) = p.finish().unwrap();
        (body, manifest(packed))
    }

    #[test]
    fn roundtrip_with_and_without_compression() {
        for opts in [
            PackOptions::default(),
            PackOptions {
                zstd_level: None,
                zstd_workers: 0,
            },
        ] {
            let (body, m) = pack(opts);
            let mut sink = MemSink::default();
            unpack(&m, &body[..], &mut sink).unwrap();
            let keys: Vec<_> = sink.0.keys().cloned().collect();
            assert_eq!(keys, ["d/", "d/A (1).TXT", "d/a.txt", "empty"]);
            assert_eq!(sink.0["d/a.txt"], b"hello");
            assert_eq!(sink.0["d/A (1).TXT"], b"other");
        }
    }

    #[test]
    fn detects_tampering() {
        let (body, mut m) = pack(PackOptions {
            zstd_level: None,
            zstd_workers: 0,
        });
        let mut bad = body.clone();
        bad[0] ^= 1;
        assert!(matches!(
            unpack(&m, &bad[..], &mut MemSink::default()),
            Err(UnpackError::HashMismatch(_))
        ));
        assert!(matches!(
            unpack(&m, &body[..body.len() - 1], &mut MemSink::default()),
            Err(UnpackError::Truncated)
        ));
        let mut longer = body.clone();
        longer.push(0);
        assert!(matches!(
            unpack(&m, &longer[..], &mut MemSink::default()),
            Err(UnpackError::TrailingData)
        ));
        m.entries[1].path = "../evil".into();
        assert!(matches!(
            unpack(&m, &body[..], &mut MemSink::default()),
            Err(UnpackError::Path(_))
        ));
    }

    #[test]
    fn text_roundtrip() {
        let (body, packed) =
            Packer::text(Vec::new(), "こんにちは", PackOptions::default()).unwrap();
        assert_eq!(
            unpack_text(&manifest(packed), &body[..]).unwrap(),
            "こんにちは"
        );
    }

    #[test]
    fn body_hasher_segments() {
        let data: Vec<u8> = (0..10_000u32).map(|i| i as u8).collect();
        let mut h = BodyHasher::new(Vec::new(), 12);
        for chunk in data.chunks(777) {
            h.write_all(chunk).unwrap();
        }
        let (out, d) = h.finish().unwrap();
        assert_eq!(out, data);
        assert_eq!(d.length, 10_000);
        assert_eq!(d.segment_blake3.len(), 3);
        assert_eq!(d.segment_blake3[2], segment_hash(&data[8192..]));
        assert_eq!(d.blake3, segment_hash(&data));
    }
}
