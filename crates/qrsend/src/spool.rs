//! Sender spool: the fully packed session on disk, so it can be streamed
//! forever (and resumed later) without re-reading or re-compressing inputs.

use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use qrsend_core::crypto::{self, DeviceIdentity, DevicePublic};
use qrsend_core::fec;
use qrsend_core::frame::FLAG_ENCRYPTED;
use qrsend_core::manifest::{Body, Kind, MANIFEST_VERSION, Manifest, MetaEnvelope, session_hex};
use qrsend_core::payload::{BodyHasher, PackOptions, Packed, Packer};
use qrsend_core::sender::{SegmentSource, SessionLayout};
use serde::{Deserialize, Serialize};

use crate::collect::{Item, Source};
use crate::{paths, util};

const MAX_AGE_SECS: i64 = 14 * 24 * 3600;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpoolInfo {
    pub session_id: u32,
    pub flags: u8,
    pub seg_shift: u8,
    pub meta_len: u32,
    pub body_len: u64,
    pub created: i64,
    pub summary: String,
    #[serde(default)]
    pub recipients: Vec<String>,
}

pub struct Spool {
    pub dir: PathBuf,
    pub info: SpoolInfo,
}

pub enum Content {
    Text(String),
    Items(Vec<Item>),
}

pub struct SpoolOptions<'a> {
    pub seg_shift: u8,
    pub pack: PackOptions,
    pub sender_name: Option<String>,
    /// Encrypt to these devices (empty = unencrypted).
    pub recipients: Vec<DevicePublic>,
    /// Sign the manifest with this identity.
    pub signer: Option<&'a DeviceIdentity>,
}

fn pack<W: Write>(w: W, content: Content, opts: PackOptions) -> Result<(W, Packed)> {
    Ok(match content {
        Content::Text(text) => Packer::text(w, &text, opts)?,
        Content::Items(items) => {
            let mut packer = Packer::new(w, opts)?;
            for item in items {
                match item.source {
                    None => packer.add_dir(&item.rel, item.mtime),
                    Some(Source::Stdin) => {
                        packer.add_file(&item.rel, &mut io::stdin().lock(), None, None, None)?;
                    }
                    Some(Source::File(path)) => {
                        let mut f = File::open(&path)
                            .with_context(|| format!("cannot open {}", path.display()))?;
                        packer
                            .add_file(&item.rel, &mut f, item.size, item.mode, item.mtime)
                            .with_context(|| format!("cannot read {}", path.display()))?;
                    }
                }
            }
            packer.finish()?
        }
    })
}

fn new_session_id() -> u32 {
    loop {
        let id: u32 = rand::random();
        if id != 0 && !paths::spools_dir().join(session_hex(id)).exists() {
            return id;
        }
    }
}

fn summary(manifest: &Manifest) -> String {
    match manifest.kind {
        Kind::Text => format!("text ({})", util::human_bytes(manifest.body.plain_length)),
        Kind::Files => {
            let files = manifest.file_count();
            let first = manifest
                .entries
                .iter()
                .find(|e| !e.path.contains('/'))
                .map_or("?".into(), |e| util::printable(&e.path));
            let tops = manifest
                .entries
                .iter()
                .filter(|e| !e.path.contains('/'))
                .count();
            let what = if tops > 1 {
                format!("{first} and {} more", tops - 1)
            } else {
                first
            };
            format!(
                "{what} ({files} file{}, {})",
                if files == 1 { "" } else { "s" },
                util::human_bytes(manifest.body.plain_length)
            )
        }
    }
}

impl Spool {
    pub fn create(content: Content, opts: &SpoolOptions) -> Result<Spool> {
        let root = paths::spools_dir();
        fs::create_dir_all(&root).with_context(|| format!("cannot create {}", root.display()))?;
        cleanup_old(&root);
        let session_id = new_session_id();
        let tmp = root.join(format!(".tmp-{}", session_hex(session_id)));
        fs::create_dir_all(&tmp)?;
        let result = Self::build(&tmp, session_id, content, opts);
        if result.is_err() {
            let _ = fs::remove_dir_all(&tmp);
        }
        let info = result?;
        let dir = root.join(session_hex(session_id));
        fs::rename(&tmp, &dir)?;
        Ok(Spool { dir, info })
    }

    fn build(
        dir: &std::path::Path,
        session_id: u32,
        content: Content,
        opts: &SpoolOptions,
    ) -> Result<SpoolInfo> {
        let body_file = BufWriter::with_capacity(1 << 20, File::create(dir.join("body.bin"))?);
        let hasher = BodyHasher::new(body_file, opts.seg_shift);
        let (hasher, packed) = if opts.recipients.is_empty() {
            pack(hasher, content, opts.pack)?
        } else {
            let enc = crypto::encrypt_writer(hasher, &opts.recipients)?;
            let (enc, packed) = pack(enc, content, opts.pack)?;
            (enc.finish()?, packed)
        };
        let (_, digest) = hasher.finish()?;
        if digest.length.div_ceil(1 << opts.seg_shift) > qrsend_core::frame::MAX_U24 as u64 {
            bail!("transfer too large for the segment size");
        }
        let manifest = Manifest {
            qrsend: MANIFEST_VERSION,
            session: session_hex(session_id),
            kind: packed.kind,
            created: util::now(),
            sender_name: opts.sender_name.clone(),
            body: Body {
                length: digest.length,
                blake3: digest.blake3,
                segment_blake3: digest.segment_blake3,
                encoding: packed.encoding,
                plain_length: packed.plain_length,
            },
            entries: packed.entries,
        };
        let mut envelope = MetaEnvelope::from_manifest(&manifest)?;
        if let Some(signer) = opts.signer {
            envelope.signature = Some(signer.sign_meta(session_id, &envelope.manifest_z));
        }
        let mut meta = envelope.encode();
        let mut flags = 0;
        if !opts.recipients.is_empty() {
            meta = crypto::encrypt(&meta, &opts.recipients)?;
            flags |= FLAG_ENCRYPTED;
        }
        fs::write(dir.join("meta.bin"), &meta)?;
        let info = SpoolInfo {
            session_id,
            flags,
            seg_shift: opts.seg_shift,
            meta_len: meta.len() as u32,
            body_len: digest.length,
            created: manifest.created,
            summary: summary(&manifest),
            recipients: opts.recipients.iter().map(|r| r.name.clone()).collect(),
        };
        fs::write(dir.join("spool.json"), serde_json::to_vec_pretty(&info)?)?;
        Ok(info)
    }

    pub fn open(session_id: u32) -> Result<Spool> {
        let dir = paths::spools_dir().join(session_hex(session_id));
        let raw = fs::read(dir.join("spool.json")).with_context(|| {
            format!("no cached send session {} on this machine (it may have expired); send the files again", session_hex(session_id))
        })?;
        Ok(Spool {
            info: serde_json::from_slice(&raw)?,
            dir,
        })
    }

    pub fn list() -> Vec<Spool> {
        let mut out: Vec<Spool> = fs::read_dir(paths::spools_dir())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let raw = fs::read(e.path().join("spool.json")).ok()?;
                Some(Spool {
                    info: serde_json::from_slice(&raw).ok()?,
                    dir: e.path(),
                })
            })
            .collect();
        out.sort_by_key(|s| s.info.created);
        out
    }

    pub fn layout(&self, symbol_size: usize) -> Result<SessionLayout> {
        let max = fec::max_segment_len(symbol_size);
        if self.info.meta_len as u64 > max {
            bail!("the file list is too large for this QR density; use a higher --density");
        }
        Ok(SessionLayout {
            session_id: self.info.session_id,
            flags: self.info.flags,
            seg_shift: self.info.seg_shift,
            meta_len: self.info.meta_len,
            body_len: self.info.body_len,
            symbol_size,
        })
    }

    pub fn source(&self) -> Result<FileSource> {
        Ok(FileSource {
            meta: fs::read(self.dir.join("meta.bin"))?,
            body: File::open(self.dir.join("body.bin"))?,
            seg_shift: self.info.seg_shift,
            body_len: self.info.body_len,
        })
    }

    pub fn disk_size(&self) -> u64 {
        self.info.body_len + self.info.meta_len as u64
    }

    pub fn remove(self) -> io::Result<()> {
        fs::remove_dir_all(self.dir)
    }
}

/// Removes spools older than two weeks (best effort).
fn cleanup_old(root: &std::path::Path) {
    let now = util::now();
    for spool in Spool::list() {
        if now - spool.info.created > MAX_AGE_SECS {
            let _ = spool.remove();
        }
    }
    // Leftovers of interrupted spool creation.
    for e in fs::read_dir(root).into_iter().flatten().flatten() {
        if e.file_name().to_string_lossy().starts_with(".tmp-") {
            let _ = fs::remove_dir_all(e.path());
        }
    }
}

pub struct FileSource {
    meta: Vec<u8>,
    body: File,
    seg_shift: u8,
    body_len: u64,
}

impl SegmentSource for FileSource {
    fn segment(&mut self, index: u32) -> io::Result<Vec<u8>> {
        if index == 0 {
            return Ok(self.meta.clone());
        }
        let nominal = 1u64 << self.seg_shift;
        let start = (index as u64 - 1) * nominal;
        let len = self.body_len.saturating_sub(start).min(nominal) as usize;
        let mut buf = vec![0u8; len];
        self.body.seek(SeekFrom::Start(start))?;
        self.body.read_exact(&mut buf)?;
        Ok(buf)
    }
}
