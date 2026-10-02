//! On-disk receive sessions (the inbox).

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use qrsend_core::manifest::session_hex;
use qrsend_core::receiver::SessionParams;
use serde::{Deserialize, Serialize};

use crate::{paths, util};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    pub session_id: u32,
    pub flags: u8,
    pub seg_shift: u8,
    pub seg_count: u32,
    /// Completed segments, as "0-5,7".
    pub done: String,
    /// Body segments completed before the manifest was known.
    #[serde(default)]
    pub unverified: String,
    pub created: i64,
    pub updated: i64,
    #[serde(default)]
    pub summary: Option<String>,
}

pub struct Store {
    pub dir: PathBuf,
    pub state: State,
    done: Vec<bool>,
    unverified: Vec<u32>,
    body: File,
}

impl Store {
    pub fn dir_for(session_id: u32) -> PathBuf {
        paths::sessions_dir().join(session_hex(session_id))
    }

    pub fn exists(session_id: u32) -> bool {
        Self::dir_for(session_id).join("state.json").exists()
    }

    pub fn params(&self) -> SessionParams {
        SessionParams {
            session_id: self.state.session_id,
            flags: self.state.flags,
            seg_shift: self.state.seg_shift,
            seg_count: self.state.seg_count,
        }
    }

    pub fn open_or_create(p: SessionParams) -> Result<Store> {
        if Self::exists(p.session_id) {
            let store = Self::open(p.session_id)?;
            if store.params() == p {
                return Ok(store);
            }
            anyhow::bail!(
                "saved session {} has different parameters; remove it with `qrsend inbox rm`",
                session_hex(p.session_id)
            );
        }
        let dir = Self::dir_for(p.session_id);
        fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
        let now = util::now();
        let state = State {
            session_id: p.session_id,
            flags: p.flags,
            seg_shift: p.seg_shift,
            seg_count: p.seg_count,
            done: String::new(),
            unverified: String::new(),
            created: now,
            updated: now,
            summary: None,
        };
        let body = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(dir.join("body.bin"))?;
        let mut store = Store {
            dir,
            state,
            done: vec![false; p.seg_count as usize + 1],
            unverified: Vec::new(),
            body,
        };
        store.save()?;
        Ok(store)
    }

    pub fn open(session_id: u32) -> Result<Store> {
        let dir = Self::dir_for(session_id);
        let raw = fs::read(dir.join("state.json"))
            .with_context(|| format!("no inbox session {}", session_hex(session_id)))?;
        let state: State = serde_json::from_slice(&raw)?;
        let mut done = vec![false; state.seg_count as usize + 1];
        for i in util::parse_ranges(&state.done)? {
            if let Some(d) = done.get_mut(i as usize) {
                *d = true;
            }
        }
        let unverified = util::parse_ranges(&state.unverified)?;
        let body = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(dir.join("body.bin"))?;
        Ok(Store {
            dir,
            state,
            done,
            unverified,
            body,
        })
    }

    /// All sessions in the inbox, oldest first.
    pub fn list() -> Vec<State> {
        let mut out: Vec<State> = fs::read_dir(paths::sessions_dir())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                serde_json::from_slice(&fs::read(e.path().join("state.json")).ok()?).ok()
            })
            .collect();
        out.sort_by_key(|s| s.created);
        out
    }

    pub fn done_indices(&self) -> Vec<u32> {
        (0..self.done.len() as u32)
            .filter(|&i| self.done[i as usize])
            .collect()
    }

    pub fn missing(&self) -> Vec<u32> {
        (0..self.done.len() as u32)
            .filter(|&i| !self.done[i as usize])
            .collect()
    }

    pub fn is_complete(&self) -> bool {
        self.done.iter().all(|&d| d) && self.unverified.is_empty()
    }

    pub fn has_meta(&self) -> bool {
        self.done[0]
    }

    pub fn meta_bytes(&self) -> Result<Vec<u8>> {
        Ok(fs::read(self.dir.join("meta.bin"))?)
    }

    pub fn write_meta(&mut self, data: &[u8]) -> Result<()> {
        let tmp = self.dir.join("meta.bin.tmp");
        fs::write(&tmp, data)?;
        fs::rename(tmp, self.dir.join("meta.bin"))?;
        self.done[0] = true;
        Ok(())
    }

    pub fn write_segment(&mut self, index: u32, data: &[u8], verified: bool) -> Result<()> {
        let offset = (index as u64 - 1) << self.state.seg_shift;
        self.body.seek(SeekFrom::Start(offset))?;
        self.body.write_all(data)?;
        self.done[index as usize] = true;
        if !verified && !self.unverified.contains(&index) {
            self.unverified.push(index);
        }
        Ok(())
    }

    pub fn read_segment(&mut self, index: u32, len: usize) -> Result<Vec<u8>> {
        let offset = (index as u64 - 1) << self.state.seg_shift;
        let mut buf = vec![0u8; len];
        self.body.seek(SeekFrom::Start(offset))?;
        self.body.read_exact(&mut buf)?;
        Ok(buf)
    }

    pub fn take_unverified(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.unverified)
    }

    pub fn forget(&mut self, index: u32) {
        self.done[index as usize] = false;
        self.unverified.retain(|&i| i != index);
    }

    pub fn body_path(&self) -> PathBuf {
        self.dir.join("body.bin")
    }

    pub fn save(&mut self) -> Result<()> {
        self.body.flush()?;
        self.state.done = util::ranges(&self.done_indices());
        let mut unverified = self.unverified.clone();
        unverified.sort_unstable();
        self.state.unverified = util::ranges(&unverified);
        self.state.updated = util::now();
        let tmp = self.dir.join("state.json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(&self.state)?)?;
        fs::rename(tmp, self.dir.join("state.json"))?;
        Ok(())
    }

    pub fn remove(self) -> Result<()> {
        drop(self.body);
        fs::remove_dir_all(&self.dir)?;
        Ok(())
    }
}
