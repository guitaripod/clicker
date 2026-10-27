//! Bonjour discovery of Apple TVs on the local network.
//!
//! `_companion-link._tcp` alone is ambiguous: the macOS Remote app advertises
//! the same service and even claims an `AppleTV` model. Real hardware also
//! publishes `_airplay._tcp`, and its AirPlay public ID (`psi`) equals the
//! Companion `rpMRtID`, so the two records are joined on that key.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

use crate::Result;

const COMPANION: &str = "_companion-link._tcp.local.";
const AIRPLAY: &str = "_airplay._tcp.local.";

#[derive(Debug, Clone, serde::Serialize)]
pub struct Discovered {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os_version: Option<String>,
    pub address: String,
    pub port: u16,
    /// The remote accepts PIN pairing.
    pub pairable: bool,
    /// Pairing is switched off on the device.
    pub pairing_disabled: bool,
    /// The record was matched to an AirPlay record on the same network, which
    /// is what a real Apple TV does. Unmatched records are usually a Mac
    /// advertising the Remote service for Continuity.
    pub airplay_confirmed: bool,
}

pub async fn scan(timeout: Duration, include_unknown: bool) -> Result<Vec<Discovered>> {
    let daemon = ServiceDaemon::new()
        .map_err(|err| crate::Error::Discovery(format!("cannot start mDNS discovery: {err}")))?;
    let companion_rx = daemon.browse(COMPANION).map_err(|err| {
        crate::Error::Discovery(format!(
            "mDNS discovery is not working on this computer, so the Apple TV cannot be found \
             (is avahi-daemon running?): {err}"
        ))
    })?;
    let airplay_rx = daemon.browse(AIRPLAY).map_err(|err| {
        crate::Error::Discovery(format!(
            "mDNS discovery is not working on this computer (is avahi-daemon running?): {err}"
        ))
    })?;

    let mut companion: HashMap<String, ServiceInfo> = HashMap::new();
    let mut airplay: HashMap<String, ServiceInfo> = HashMap::new();

    let collect = async {
        loop {
            tokio::select! {
                event = companion_rx.recv_async() => match event {
                    Ok(ServiceEvent::ServiceResolved(info)) => {
                        let key = instance(&info);
                        companion.insert(key, info);
                    }
                    Ok(ServiceEvent::ServiceRemoved(_, fullname)) => {
                        companion.remove(&fullname);
                    }
                    Ok(_) => {}
                    Err(_) => return,
                },
                event = airplay_rx.recv_async() => match event {
                    Ok(ServiceEvent::ServiceResolved(info)) => {
                        let key = instance(&info);
                        airplay.insert(key, info);
                    }
                    Ok(ServiceEvent::ServiceRemoved(_, fullname)) => {
                        airplay.remove(&fullname);
                    }
                    Ok(_) => {}
                    Err(_) => return,
                },
            }
        }
    };
    let _ = tokio::time::timeout(timeout, collect).await;
    let _ = daemon.shutdown();

    let airplay_by_id: HashMap<String, &ServiceInfo> = airplay
        .values()
        .filter_map(|info| {
            let psi = info.get_property_val_str("psi")?;
            Some((psi.to_ascii_uppercase(), info))
        })
        .collect();

    let mut found = Vec::new();
    for info in companion.values() {
        let rp_id = txt(info, "rpMRtID").map(|value| value.to_ascii_uppercase());
        let paired_hint = rp_id.as_ref().and_then(|id| airplay_by_id.get(id)).copied();
        let model = txt(info, "rpMd").map(str::to_string);
        let claims_media_device = model.as_deref().map(is_media_device).unwrap_or(false);

        // A companion record whose AirPlay twin is a Mac or a TV is the Remote
        // app's own advertisement; a record without an Apple TV model is not a
        // controllable device either.
        if !claims_media_device && !include_unknown {
            continue;
        }
        let airplay_confirmed = paired_hint
            .and_then(|airplay_info| airplay_info.get_property_val_str("model"))
            .map(is_media_device)
            .unwrap_or(false);

        let flags = txt(info, "rpFl")
            .and_then(|value| u32::from_str_radix(value.trim_start_matches("0x"), 16).ok())
            .unwrap_or(0);
        let Some(address) = preferred_address(info) else {
            continue;
        };

        found.push(Discovered {
            id: rp_id
                .map(|id| id.replace('-', "").to_ascii_lowercase())
                .or_else(|| {
                    txt(info, "rpBA").map(|value| value.replace(':', "").to_ascii_lowercase())
                })
                .unwrap_or_else(|| instance(info)),
            name: instance_name(info),
            model,
            os_version: paired_hint
                .and_then(|airplay_info| airplay_info.get_property_val_str("osvers"))
                .map(str::to_string),
            address,
            port: info.get_port(),
            pairable: flags & 0x4000 != 0,
            pairing_disabled: flags & 0x04 != 0,
            airplay_confirmed,
        });
    }

    found.sort_by(|a, b| {
        b.airplay_confirmed
            .cmp(&a.airplay_confirmed)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    found.dedup_by(|a, b| a.id == b.id);
    Ok(found)
}

fn is_media_device(model: &str) -> bool {
    model.starts_with("AppleTV") || model.starts_with("AudioAccessory")
}

fn txt<'a>(info: &'a ServiceInfo, key: &str) -> Option<&'a str> {
    info.get_property_val_str(key)
        .filter(|value| !value.is_empty())
}

fn preferred_address(info: &ServiceInfo) -> Option<String> {
    let mut v4: Vec<&IpAddr> = info.get_addresses().iter().collect();
    v4.sort_by_key(|addr| if addr.is_ipv4() { 0 } else { 1 });
    v4.first().map(|addr| addr.to_string())
}

fn instance(info: &ServiceInfo) -> String {
    info.get_fullname().to_string()
}

fn instance_name(info: &ServiceInfo) -> String {
    let fullname = info.get_fullname();
    fullname
        .split("._")
        .next()
        .unwrap_or(fullname)
        .trim_end_matches('.')
        .to_string()
}
