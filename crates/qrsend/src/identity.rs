//! This device's identity and the trusted-devices list.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use qrsend_core::crypto::{DeviceIdentity, DevicePublic};
use serde::{Deserialize, Serialize};

use crate::{paths, util};

pub fn identity_path() -> PathBuf {
    paths::config_dir().join("identity")
}

pub fn devices_path() -> PathBuf {
    paths::config_dir().join("devices.json")
}

pub fn load() -> Result<Option<DeviceIdentity>> {
    match fs::read_to_string(identity_path()) {
        Ok(text) => {
            let mut identity = DeviceIdentity::parse(&text)?;
            // An identity from before post-quantum keys is given them the
            // first time it is used (docs/POST-QUANTUM.md). Its other keys
            // stay, so nothing changes for devices that know it; if the
            // file cannot be written, it goes on without the new keys.
            if !identity.has_pq() {
                let mut upgraded = DeviceIdentity::parse(&text)?;
                if upgraded.upgrade()? && save(&upgraded).is_ok() {
                    eprintln!(
                        "This device's identity now has post-quantum keys as well. Devices that \
                         trust it go on as before; they use the new keys once they are given its \
                         ID again (`qrsend id`)."
                    );
                    identity = upgraded;
                }
            }
            Ok(Some(identity))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).context("cannot read the device identity"),
    }
}

pub fn default_name() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().trim_end_matches(".local").to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "qrsend-device".into())
}

pub fn save(identity: &DeviceIdentity) -> Result<()> {
    let path = identity_path();
    paths::create_private(path.parent().unwrap())?;
    // (Never readable by others, not even for a moment.)
    paths::write_private(&path, identity.to_secret_string().as_bytes())?;
    Ok(())
}

/// Loads the identity, creating one on first use.
pub fn load_or_create(name: Option<&str>) -> Result<(DeviceIdentity, bool)> {
    if let Some(id) = load()? {
        return Ok((id, false));
    }
    let id = DeviceIdentity::generate(
        name.map(str::to_string)
            .unwrap_or_else(default_name)
            .as_str(),
    )?;
    save(&id)?;
    Ok((id, true))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trusted {
    pub id: String,
    pub name: String,
    pub added: i64,
}

impl Trusted {
    pub fn public(&self) -> Result<DevicePublic> {
        let mut p = DevicePublic::parse(&self.id)?;
        p.name = self.name.clone();
        Ok(p)
    }
}

pub fn devices() -> Result<Vec<Trusted>> {
    match fs::read(devices_path()) {
        Ok(raw) => Ok(serde_json::from_slice(&raw).context("devices.json is corrupt")?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

pub fn save_devices(list: &[Trusted]) -> Result<()> {
    let path = devices_path();
    paths::create_private(path.parent().unwrap())?;
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(list)?)?;
    fs::rename(tmp, path)?;
    Ok(())
}

/// What adding a device came to.
pub enum Added {
    New(Trusted),
    /// Trusted before with its older keys only: the record now holds its
    /// post-quantum keys as well.
    Upgraded(Trusted),
}

pub fn add_device(public: &DevicePublic, name: Option<&str>) -> Result<Added> {
    let mut list = devices()?;
    // The same device as one already trusted, going by the keys it had
    // from the start.
    let known = list
        .iter()
        .position(|d| d.public().is_ok_and(|p| p.classic() == public.classic()));
    if let Some(at) = known {
        let existing = list[at].public()?;
        return match (&existing.pq, &public.pq) {
            (None, Some(_)) => {
                list[at].id = public.to_id_string();
                if let Some(name) = name {
                    list[at].name = name.to_string();
                }
                let t = list[at].clone();
                save_devices(&list)?;
                Ok(Added::Upgraded(t))
            }
            (Some(a), Some(b)) if a != b => bail!(
                "{:?} is trusted with other post-quantum keys than this ID shows. If the device \
                 made itself new keys, remove it first (`qrsend devices rm`); if not, this is \
                 not its ID",
                list[at].name
            ),
            (Some(_), None) => bail!(
                "this device is already trusted as {:?}, with post-quantum keys; this is its \
                 older ID, which adds nothing",
                list[at].name
            ),
            _ => bail!("this device is already trusted as {:?}", list[at].name),
        };
    }
    let name = name.unwrap_or(&public.name).to_string();
    if list.iter().any(|d| d.name == name) {
        bail!("a trusted device is already named {name:?}; pass --name to choose another");
    }
    let t = Trusted {
        id: public.to_id_string(),
        name,
        added: util::now(),
    };
    list.push(t.clone());
    save_devices(&list)?;
    Ok(Added::New(t))
}

/// Finds a trusted device by name or fingerprint, or parses a raw ID string.
pub fn resolve(spec: &str) -> Result<DevicePublic> {
    if spec.starts_with("qrsend-id:") {
        return Ok(DevicePublic::parse(spec)?);
    }
    let list = devices()?;
    for d in &list {
        let p = d.public()?;
        if d.name == spec || p.fingerprint() == spec || p.classic().fingerprint() == spec {
            return Ok(p);
        }
    }
    let known: Vec<&str> = list.iter().map(|d| d.name.as_str()).collect();
    if known.is_empty() {
        bail!(
            "no trusted device {spec:?}; add one with `qrsend devices add <ID>` (run `qrsend id` on the receiver)"
        );
    }
    bail!("no trusted device {spec:?} (known: {})", known.join(", "))
}

/// Whether an ID given in full (`qrsend-id:…`) is that of a trusted device.
pub fn is_trusted(id: &str) -> bool {
    let Ok(given) = DevicePublic::parse(id) else {
        return false;
    };
    devices().is_ok_and(|list| list.iter().any(|d| d.public().is_ok_and(|p| p == given)))
}

/// The trusted device with this signing key, if any (under the name it is
/// trusted as).
pub fn trusted_device(verifying: &[u8; 32]) -> Option<DevicePublic> {
    devices().ok()?.into_iter().find_map(|d| {
        d.public()
            .ok()
            .filter(|p| p.verifying.to_bytes() == *verifying)
    })
}
