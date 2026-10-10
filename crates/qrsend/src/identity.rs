//! This device's identity and the trusted-devices list.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use qrsend_core::crypto::{DeviceIdentity, DevicePublic};
use serde::{Deserialize, Serialize};

use crate::{paths, util};

fn identity_path() -> PathBuf {
    paths::config_dir().join("identity")
}

fn devices_path() -> PathBuf {
    paths::config_dir().join("devices.json")
}

pub fn load() -> Result<Option<DeviceIdentity>> {
    match fs::read_to_string(identity_path()) {
        Ok(text) => Ok(Some(DeviceIdentity::parse(&text)?)),
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

pub fn add_device(public: &DevicePublic, name: Option<&str>) -> Result<Trusted> {
    let mut list = devices()?;
    let name = name.unwrap_or(&public.name).to_string();
    if list.iter().any(|d| d.name == name) {
        bail!("a trusted device is already named {name:?}; pass --name to choose another");
    }
    let fpr = public.fingerprint();
    if let Some(existing) = list
        .iter()
        .find(|d| d.public().map(|p| p.fingerprint() == fpr).unwrap_or(false))
    {
        bail!("this device is already trusted as {:?}", existing.name);
    }
    let t = Trusted {
        id: public.to_id_string(),
        name,
        added: util::now(),
    };
    list.push(t.clone());
    save_devices(&list)?;
    Ok(t)
}

/// Finds a trusted device by name or fingerprint, or parses a raw ID string.
pub fn resolve(spec: &str) -> Result<DevicePublic> {
    if spec.starts_with("qrsend-id:") {
        return Ok(DevicePublic::parse(spec)?);
    }
    let list = devices()?;
    for d in &list {
        let p = d.public()?;
        if d.name == spec || p.fingerprint() == spec {
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

/// Name of the trusted device with this signing key, if any.
pub fn trusted_name(verifying: &[u8; 32]) -> Option<String> {
    devices()
        .ok()?
        .into_iter()
        .find(|d| {
            d.public()
                .is_ok_and(|p| p.verifying.to_bytes() == *verifying)
        })
        .map(|d| d.name)
}
