//! Shared command context: the store, the output sink and connection recovery.

use std::time::Duration;

use serde_json::{json, Value};

use crate::companion::client::Client;
use crate::discovery;
use crate::output::Output;
use crate::store::{PairedDevice, Store};
use crate::{Error, Result};

pub struct Context {
    pub out: Output,
    pub store: Store,
    pub selector: Option<String>,
    pub timeout: Duration,
}

impl Context {
    pub fn device_json(device: &PairedDevice) -> Value {
        json!({
            "id": device.id,
            "name": device.name,
            "model": device.model,
            "address": device.address,
            "port": device.port,
        })
    }

    pub fn resolve(&self) -> Result<PairedDevice> {
        match &self.selector {
            Some(selector) => self.store.find(selector).cloned(),
            None => {
                if self.store.devices().is_empty() {
                    return Err(Error::NotPaired(
                        "no Apple TV is paired on this computer".into(),
                    ));
                }
                self.store.only().cloned()
            }
        }
    }

    /// Connect to the selected device, rediscovering it if its address moved.
    pub async fn connect(&mut self) -> Result<(PairedDevice, Client)> {
        let device = self.resolve()?;
        let credentials = device.credentials()?;
        match Client::connect(&device.address, device.port, &credentials, self.timeout).await {
            Ok(client) => Ok((device, client)),
            Err(first) => match self.rediscover(&device).await? {
                Some(moved) => {
                    self.out
                        .note(format_args!("{} moved to {}", moved.name, moved.address));
                    let credentials = moved.credentials()?;
                    let client =
                        Client::connect(&moved.address, moved.port, &credentials, self.timeout)
                            .await?;
                    Ok((moved, client))
                }
                None => Err(first),
            },
        }
    }

    pub async fn rediscover(&mut self, device: &PairedDevice) -> Result<Option<PairedDevice>> {
        let found = discovery::scan(Duration::from_secs(2), false).await?;
        let Some(candidate) = found.iter().find(|candidate| candidate.id == device.id) else {
            return Ok(None);
        };
        if candidate.address == device.address && candidate.port == device.port {
            return Ok(None);
        }
        let mut updated = device.clone();
        updated.address = candidate.address.clone();
        updated.port = candidate.port;
        if let Some(model) = &candidate.model {
            updated.model = Some(model.clone());
        }
        self.store.upsert(updated.clone());
        self.store.save()?;
        Ok(Some(updated))
    }
}
