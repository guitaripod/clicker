//! Paired-device store. Credentials are private keys, so the file is 0600.

use std::fs;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::discovery::Discovered;
use crate::hap::Credentials;
use crate::{Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedDevice {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub address: String,
    pub port: u16,
    /// `ltpk:ltsk:atv_id:client_id`, hex, identical to pyatv's format.
    pub credentials: String,
    #[serde(default = "default_display_name")]
    pub display_name: String,
    #[serde(default)]
    pub paired_at: String,
}

fn default_display_name() -> String {
    "Clicker".into()
}

impl PairedDevice {
    pub fn credentials(&self) -> Result<Credentials> {
        Credentials::from_pairing_string(&self.credentials)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Document {
    #[serde(default)]
    devices: Vec<PairedDevice>,
}

#[derive(Debug)]
pub struct Store {
    path: PathBuf,
    devices: Vec<PairedDevice>,
}

impl Store {
    pub fn load() -> Result<Self> {
        let path = store_path()?;
        if !path.exists() {
            return Ok(Self {
                path,
                devices: Vec::new(),
            });
        }
        let raw = fs::read_to_string(&path)?;
        let document: Document = serde_json::from_str(&raw)
            .map_err(|err| Error::Config(format!("{} is corrupt: {err}", path.display())))?;
        Ok(Self {
            path,
            devices: document.devices,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn devices(&self) -> &[PairedDevice] {
        &self.devices
    }

    /// Resolve a selector by exact id, id prefix, name, model, or address.
    pub fn find(&self, selector: &str) -> Result<&PairedDevice> {
        let needle = selector.trim().to_ascii_lowercase();
        if let Some(found) = self.devices.iter().find(|device| {
            device.id.to_ascii_lowercase() == needle
                || device.name.to_ascii_lowercase() == needle
                || device.address == needle
        }) {
            return Ok(found);
        }
        let matches: Vec<&PairedDevice> = self
            .devices
            .iter()
            .filter(|device| {
                device.id.to_ascii_lowercase().starts_with(&needle)
                    || device.name.to_ascii_lowercase().contains(&needle)
            })
            .collect();
        match matches.len() {
            1 => Ok(matches[0]),
            0 => Err(Error::DeviceNotFound(selector.to_string())),
            _ => {
                let names: Vec<&str> = matches.iter().map(|d| d.name.as_str()).collect();
                Err(Error::Usage(format!(
                    "\"{selector}\" matches several devices: {}",
                    names.join(", ")
                )))
            }
        }
    }

    pub fn only(&self) -> Result<&PairedDevice> {
        match self.devices.len() {
            1 => Ok(&self.devices[0]),
            0 => Err(Error::NotPaired(
                "no Apple TV is paired on this computer".into(),
            )),
            _ => {
                let names: Vec<String> = self
                    .devices
                    .iter()
                    .map(|d| format!("{} ({})", d.name, d.id))
                    .collect();
                Err(Error::Usage(format!(
                    "several devices are paired; pass --device: {}",
                    names.join(", ")
                )))
            }
        }
    }

    pub fn upsert(&mut self, device: PairedDevice) {
        match self
            .devices
            .iter_mut()
            .find(|existing| existing.id == device.id)
        {
            Some(existing) => *existing = device,
            None => self.devices.push(device),
        }
    }

    pub fn remove(&mut self, selector: &str) -> Result<PairedDevice> {
        let id = self.find(selector)?.id.clone();
        let index = self
            .devices
            .iter()
            .position(|device| device.id == id)
            .expect("device found above");
        Ok(self.devices.remove(index))
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
            set_mode(parent, 0o700)?;
        }
        let document = Document {
            devices: self.devices.clone(),
        };
        let encoded = serde_json::to_string_pretty(&document)
            .map_err(|err| Error::Config(format!("cannot encode device store: {err}")))?;
        let temp = self.path.with_extension("json.tmp");
        {
            let mut file = fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .mode(0o600)
                .open(&temp)?;
            use std::io::Write;
            file.write_all(encoded.as_bytes())?;
            file.sync_all()?;
        }
        set_mode(&temp, 0o600)?;
        fs::rename(&temp, &self.path)?;
        Ok(())
    }

    pub fn from_discovery(
        discovered: &Discovered,
        credentials: Credentials,
        display_name: &str,
    ) -> PairedDevice {
        PairedDevice {
            id: discovered.id.clone(),
            name: discovered.name.clone(),
            model: discovered.model.clone(),
            address: discovered.address.clone(),
            port: discovered.port,
            credentials: credentials.to_pairing_string(),
            display_name: display_name.to_string(),
            paired_at: now(),
        }
    }
}

fn store_path() -> Result<PathBuf> {
    if let Some(dir) = env_dir("CLICKER_CONFIG_DIR") {
        return Ok(dir.join("devices.json"));
    }
    if let Some(dir) = env_dir("XDG_CONFIG_HOME") {
        return Ok(dir.join("clicker/devices.json"));
    }
    Ok(config_root()?.join("clicker/devices.json"))
}

/// Follow the platform convention: `~/Library/Application Support` on macOS,
/// `$XDG_CONFIG_HOME` (or `~/.config`) everywhere else.
fn config_root() -> Result<PathBuf> {
    if let Some(dir) = env_dir("XDG_CONFIG_HOME") {
        return Ok(dir);
    }
    let home =
        env_dir("HOME").ok_or_else(|| Error::Config("cannot locate home directory".into()))?;
    if cfg!(target_os = "macos") {
        Ok(home.join("Library/Application Support"))
    } else {
        Ok(home.join(".config"))
    }
}

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    Ok(())
}

fn now() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{seconds}")
}
