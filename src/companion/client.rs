//! Companion client: session setup, commands, events.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::net::tcp::OwnedReadHalf;
use tokio::sync::{broadcast, mpsc, oneshot};

use super::conn::{writer_loop, Connection, Outbound};
use super::frame::{payload_len, FrameType, HEADER_LEN};
use super::keys::{Gesture, HidCommand, InputAction, MediaCommand, PowerState, TouchPhase};
use crate::crypto::Direction;
use crate::hap::Credentials;
use crate::opack::{self, Value};
use crate::text::{self, TextInput};
use crate::{Error, Result};

const DEFAULT_GAP_MS: u64 = 30;
const DEFAULT_HOLD_MS: u64 = 1000;
const TOUCHPAD_MAX: f64 = 1000.0;

#[derive(Debug, Clone)]
pub struct Event {
    pub name: String,
    pub content: Value,
}

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub model: Option<String>,
    pub os_version: Option<String>,
    pub name: Option<String>,
}

pub struct Client {
    tx: mpsc::Sender<Outbound>,
    pending: Arc<Mutex<HashMap<u16, oneshot::Sender<Value>>>>,
    events: broadcast::Sender<Event>,
    next_xid: AtomicU16,
    timeout: Duration,
    pub sid: u64,
    pub info: DeviceInfo,
    /// Media-control capability flags pushed by the device at connect.
    pub media_flags: Option<i64>,
    alive: Arc<AtomicBool>,
}

impl Client {
    pub async fn connect(
        address: &str,
        port: u16,
        credentials: &Credentials,
        timeout: Duration,
    ) -> Result<Self> {
        let mut conn = Connection::open(address, port, timeout).await?;
        let (out_key, in_key) = crate::hap::verify(&mut conn, credentials).await?;
        conn.enable_encryption(out_key, in_key);

        let (reader, writer, buffer, out_cipher, in_cipher) = conn.split();
        let pending: Arc<Mutex<HashMap<u16, oneshot::Sender<Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (events, _) = broadcast::channel(64);
        let (tx, rx) = mpsc::channel(32);
        let alive = Arc::new(AtomicBool::new(true));

        tokio::spawn(writer_loop(writer, out_cipher, rx));
        tokio::spawn(reader_loop(
            reader,
            buffer,
            in_cipher,
            pending.clone(),
            events.clone(),
            alive.clone(),
        ));

        let mut client = Self {
            tx,
            pending,
            events,
            next_xid: AtomicU16::new(rand::random::<u16>()),
            timeout,
            sid: 0,
            info: DeviceInfo {
                model: None,
                os_version: None,
                name: None,
            },
            media_flags: None,
            alive,
        };

        let info = client.system_info(credentials).await?;
        client.info = DeviceInfo {
            model: info
                .get("_siriInfo")
                .and_then(|siri| siri.get("peerData"))
                .and_then(|peer| peer.get("productType"))
                .and_then(Value::as_str)
                .or_else(|| {
                    info.get("model")
                        .and_then(Value::as_str)
                        .filter(|model| *model != "peerData")
                })
                .map(str::to_string),
            os_version: info.get("_osV").and_then(Value::as_str).map(str::to_string),
            name: info.get("name").and_then(Value::as_str).map(str::to_string),
        };

        client
            .request(
                "_touchStart",
                dict(vec![
                    ("_height", Value::Float(TOUCHPAD_MAX)),
                    ("_tFl", Value::Int(0)),
                    ("_width", Value::Float(TOUCHPAD_MAX)),
                ]),
            )
            .await?;

        client.sid = client.start_session().await?;

        // tvOS 26.5+ only serves app lists and attention state once a TV Remote
        // Client session exists. Older versions ignore or refuse it.
        let _ = client
            .request(
                "TVRCSessionStart",
                dict(vec![("ProtocolVersionKey", Value::Str("1.2".into()))]),
            )
            .await;

        let _ = client.request("_tiStart", dict(vec![])).await;

        let mut events = client.events();
        client
            .subscribe(&[
                "_iMC",
                "SystemStatus",
                "TVSystemStatus",
                "_tiStarted",
                "_tiStopped",
            ])
            .await?;

        // The device pushes its media-control flags right after registration;
        // capturing them here saves every later command a round trip.
        let deadline = tokio::time::Instant::now() + Duration::from_millis(750);
        while let Ok(Ok(event)) = tokio::time::timeout_at(deadline, events.recv()).await {
            if event.name == "_iMC" {
                client.media_flags = event.content.get("_mcF").and_then(Value::as_i64);
                break;
            }
        }

        crate::log::info(format_args!(
            "connected, session 0x{:x}, tvOS {}",
            client.sid,
            client.info.os_version.as_deref().unwrap_or("?")
        ));
        Ok(client)
    }

    pub fn events(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    pub async fn request(&self, identifier: &str, content: Value) -> Result<Value> {
        let xid = self.next_xid.fetch_add(1, Ordering::SeqCst);
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().unwrap().insert(xid, sender);

        let message = dict(vec![
            ("_i", Value::Str(identifier.to_string())),
            ("_t", Value::Int(2)),
            ("_x", Value::Int(xid as i64)),
            ("_c", content),
        ]);
        self.send_opack(message)?;

        crate::log::debug(format_args!("request {identifier}"));
        match tokio::time::timeout(self.timeout, receiver).await {
            Ok(Ok(response)) => {
                if let Some(error) = response.get("_em") {
                    return Err(Error::Protocol(format!(
                        "the Apple TV rejected {identifier}: {}",
                        error
                    )));
                }
                Ok(response)
            }
            Ok(Err(_)) => Err(Error::Protocol("connection closed mid-request".into())),
            Err(_) => {
                self.pending.lock().unwrap().remove(&xid);
                Err(Error::Timeout(self.timeout))
            }
        }
    }

    pub async fn send_event(&self, identifier: &str, content: Value) -> Result<()> {
        let message = dict(vec![
            ("_i", Value::Str(identifier.to_string())),
            ("_t", Value::Int(1)),
            ("_c", content),
        ]);
        self.send_opack(message)
    }

    fn send_opack(&self, message: Value) -> Result<()> {
        if !self.alive.load(Ordering::SeqCst) {
            return Err(Error::Protocol("connection closed by the Apple TV".into()));
        }
        let payload = opack::pack(&message)?;
        self.tx
            .try_send(Outbound {
                frame_type: FrameType::EOpack,
                payload,
            })
            .map_err(|err| Error::Protocol(format!("cannot queue frame: {err}")))
    }

    async fn system_info(&self, credentials: &Credentials) -> Result<Value> {
        let client_hex: String = credentials
            .client_id
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
            .chars()
            .filter(|c| c.is_ascii_hexdigit())
            .collect();
        let rp_id: String = client_hex.chars().take(12).collect();

        let content = dict(vec![
            ("_bf", Value::Int(0)),
            ("_cf", Value::Int(512)),
            ("_clFl", Value::Int(128)),
            ("_i", Value::Str(rp_id)),
            ("_idsID", Value::Bytes(credentials.client_id.clone())),
            ("_pubID", Value::Str(public_id(credentials))),
            ("_sf", Value::Int(256)),
            ("_sv", Value::Str("170.18".into())),
            ("model", Value::Str("iPhone10,6".into())),
            ("name", Value::Str("Clicker".into())),
        ]);
        let response = self.request("_systemInfo", content).await?;
        Ok(response.get("_c").cloned().unwrap_or(Value::Null))
    }

    async fn start_session(&self) -> Result<u64> {
        let local: u32 = rand::random();
        let response = self
            .request(
                "_sessionStart",
                dict(vec![
                    ("_srvT", Value::Str("com.apple.tvremoteservices".into())),
                    ("_sid", Value::Int(local as i64)),
                ]),
            )
            .await?;
        let remote = response
            .get("_c")
            .and_then(|content| content.get("_sid"))
            .and_then(Value::as_i64)
            .ok_or_else(|| Error::Protocol("session start returned no session id".into()))?;
        Ok(((remote as u64) << 32) | local as u64)
    }

    pub async fn subscribe(&self, names: &[&str]) -> Result<()> {
        let events = names
            .iter()
            .map(|name| Value::Str((*name).to_string()))
            .collect();
        self.send_event(
            "_interest",
            dict(vec![("_regEvents", Value::Array(events))]),
        )
        .await
    }

    pub async fn wait_for_event(&self, name: &str, timeout: Duration) -> Option<Value> {
        let mut receiver = self.events.subscribe();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            match tokio::time::timeout_at(deadline, receiver.recv()).await {
                Ok(Ok(event)) if event.name == name => return Some(event.content),
                Ok(Ok(_)) => continue,
                Ok(Err(_)) | Err(_) => return None,
            }
        }
    }

    pub async fn hid(&self, down: bool, command: HidCommand) -> Result<()> {
        self.request(
            "_hidC",
            dict(vec![
                ("_hBtS", Value::Int(if down { 1 } else { 2 })),
                ("_hidC", Value::Int(command as i64)),
            ]),
        )
        .await?;
        Ok(())
    }

    pub async fn press(
        &self,
        command: HidCommand,
        action: InputAction,
        hold: Duration,
    ) -> Result<()> {
        let gap = Duration::from_millis(DEFAULT_GAP_MS);
        match action {
            InputAction::Single => {
                self.hid(true, command).await?;
                tokio::time::sleep(gap).await;
                self.hid(false, command).await?;
            }
            InputAction::Double => {
                for _ in 0..2 {
                    self.hid(true, command).await?;
                    tokio::time::sleep(gap).await;
                    self.hid(false, command).await?;
                    tokio::time::sleep(gap).await;
                }
            }
            InputAction::Hold => {
                self.hid(true, command).await?;
                tokio::time::sleep(hold).await;
                self.hid(false, command).await?;
            }
        }
        Ok(())
    }

    pub async fn hold_for(&self, command: HidCommand, hold: Duration) -> Result<()> {
        self.press(command, InputAction::Hold, hold).await
    }

    pub async fn media(&self, command: MediaCommand, extra: Vec<(&str, Value)>) -> Result<Value> {
        let mut content = dict(vec![("_mcc", Value::Int(command as i64))]);
        if let Value::Dict(pairs) = &mut content {
            for (key, value) in extra {
                pairs.push((key.to_string(), value));
            }
        }
        self.request("_mcc", content).await
    }

    pub async fn volume(&self) -> Result<Option<f64>> {
        let response = self.media(MediaCommand::GetVolume, vec![]).await?;
        Ok(response
            .get("_c")
            .and_then(|content| content.get("_vol"))
            .and_then(Value::as_f64)
            .map(|level| ((level * 1000.0).round() / 10.0).clamp(0.0, 100.0)))
    }

    pub async fn set_volume(&self, percent: f64) -> Result<()> {
        self.media(
            MediaCommand::SetVolume,
            vec![("_vol", Value::Float(percent / 100.0))],
        )
        .await?;
        Ok(())
    }

    pub async fn skip(&self, seconds: f64) -> Result<()> {
        self.media(MediaCommand::SkipBy, vec![("_skpS", Value::Float(seconds))])
            .await?;
        Ok(())
    }

    pub async fn power_state(&self) -> PowerState {
        match self.request("FetchAttentionState", dict(vec![])).await {
            Ok(response) => response
                .get("_c")
                .and_then(|content| content.get("state"))
                .and_then(Value::as_i64)
                .map(PowerState::from_attention_state)
                .unwrap_or(PowerState::Unknown),
            Err(_) => PowerState::Unknown,
        }
    }

    pub async fn media_flags(&self, wait: Duration) -> Option<i64> {
        match self.wait_for_event("_iMC", wait).await {
            Some(content) => content.get("_mcF").and_then(Value::as_i64),
            None => None,
        }
    }

    pub async fn app_list(&self) -> Result<Vec<(String, String)>> {
        let response = self
            .request("FetchLaunchableApplicationsEvent", dict(vec![]))
            .await?;
        let content = response
            .get("_c")
            .and_then(Value::as_dict)
            .ok_or_else(|| Error::Protocol("this tvOS does not report launchable apps".into()))?;
        let mut apps: Vec<(String, String)> = content
            .iter()
            .filter_map(|(bundle, name)| {
                name.as_str().map(|name| (bundle.clone(), name.to_string()))
            })
            .collect();
        apps.sort_by_key(|(_, name)| name.to_lowercase());
        Ok(apps)
    }

    pub async fn launch_app(&self, target: &str) -> Result<()> {
        let key = if target.contains("://") {
            "_urlS"
        } else {
            "_bundleID"
        };
        self.request(
            "_launchApp",
            dict(vec![(key, Value::Str(target.to_string()))]),
        )
        .await?;
        Ok(())
    }

    pub async fn account_list(&self) -> Result<Vec<(String, String)>> {
        let response = self.request("FetchUserAccountsEvent", dict(vec![])).await?;
        let content = response
            .get("_c")
            .and_then(Value::as_dict)
            .ok_or_else(|| Error::Protocol("this tvOS does not report user accounts".into()))?;
        Ok(content
            .iter()
            .filter_map(|(id, name)| name.as_str().map(|name| (id.clone(), name.to_string())))
            .collect())
    }

    pub async fn switch_account(&self, id: &str) -> Result<()> {
        self.request(
            "SwitchUserAccountEvent",
            dict(vec![("SwitchAccountID", Value::Str(id.to_string()))]),
        )
        .await?;
        Ok(())
    }

    /// Restart the RTI session and return its handle plus current text.
    pub async fn text_session(&self) -> Result<TextInput> {
        let _ = self.request("_tiStop", dict(vec![])).await;
        let response = self.request("_tiStart", dict(vec![])).await?;
        let archive = response
            .get("_c")
            .and_then(|content| content.get("_tiD"))
            .and_then(|value| match value {
                Value::Bytes(bytes) => Some(bytes.clone()),
                _ => None,
            })
            .ok_or_else(|| {
                Error::Protocol(
                    "no text field is focused on the Apple TV; open a search field first".into(),
                )
            })?;
        text::parse_text_input(&archive)
    }

    pub async fn text_set(&self, value: &str) -> Result<String> {
        let session = self.text_session().await?;
        let mut base = session.current_text.clone();
        if !base.is_empty() {
            let payload = text::clear_payload(&session.session_uuid)?;
            self.send_event(
                "_tiC",
                dict(vec![
                    ("_tiV", Value::Int(1)),
                    ("_tiD", Value::Bytes(payload)),
                ]),
            )
            .await?;
            base.clear();
        }
        self.insert_text(&session, value, &base).await
    }

    pub async fn text_append(&self, value: &str) -> Result<String> {
        let session = self.text_session().await?;
        let base = session.current_text.clone();
        self.insert_text(&session, value, &base).await
    }

    pub async fn text_clear(&self) -> Result<()> {
        let session = self.text_session().await?;
        let payload = text::clear_payload(&session.session_uuid)?;
        self.send_event(
            "_tiC",
            dict(vec![
                ("_tiV", Value::Int(1)),
                ("_tiD", Value::Bytes(payload)),
            ]),
        )
        .await?;
        Ok(())
    }

    pub async fn text_get(&self) -> Result<String> {
        Ok(self.text_session().await?.current_text)
    }

    async fn insert_text(&self, session: &TextInput, value: &str, base: &str) -> Result<String> {
        if value.is_empty() {
            return Ok(base.to_string());
        }
        let payload = text::input_payload(&session.session_uuid, value)?;
        self.send_event(
            "_tiC",
            dict(vec![
                ("_tiV", Value::Int(1)),
                ("_tiD", Value::Bytes(payload)),
            ]),
        )
        .await?;
        let mut text = base.to_string();
        text.push_str(value);
        Ok(text)
    }

    pub async fn touch(&self, x: i64, y: i64, phase: TouchPhase) -> Result<()> {
        let x = x.clamp(0, TOUCHPAD_MAX as i64);
        let y = y.clamp(0, TOUCHPAD_MAX as i64);
        let elapsed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        self.send_event(
            "_hidT",
            dict(vec![
                ("_ns", Value::Int(elapsed)),
                ("_tFg", Value::Int(1)),
                ("_cx", Value::Int(x)),
                ("_tPh", Value::Int(phase as i64)),
                ("_cy", Value::Int(y)),
            ]),
        )
        .await
    }

    pub async fn swipe(&self, gesture: Gesture) -> Result<()> {
        self.touch(gesture.from.0, gesture.from.1, TouchPhase::Press)
            .await?;
        let step = Duration::from_millis(16);
        let steps = (gesture.duration.as_millis() / 16).max(1) as i64;
        for i in 1..steps {
            let ratio = i as f64 / steps as f64;
            let x = gesture.from.0 as f64 + (gesture.to.0 - gesture.from.0) as f64 * ratio;
            let y = gesture.from.1 as f64 + (gesture.to.1 - gesture.from.1) as f64 * ratio;
            self.touch(x.round() as i64, y.round() as i64, TouchPhase::Hold)
                .await?;
            tokio::time::sleep(step).await;
        }
        self.touch(gesture.to.0, gesture.to.1, TouchPhase::Release)
            .await
    }

    pub async fn click(&self, action: InputAction) -> Result<()> {
        let press = HidCommand::Select as i64;
        let hold = match action {
            InputAction::Hold => Duration::from_millis(DEFAULT_HOLD_MS),
            _ => Duration::from_millis(20),
        };
        let times = if action == InputAction::Double { 2 } else { 1 };
        for _ in 0..times {
            self.request(
                "_hidC",
                dict(vec![("_hBtS", Value::Int(1)), ("_hidC", Value::Int(press))]),
            )
            .await?;
            tokio::time::sleep(hold).await;
            self.request(
                "_hidC",
                dict(vec![("_hBtS", Value::Int(2)), ("_hidC", Value::Int(press))]),
            )
            .await?;
            self.touch(TOUCHPAD_MAX as i64, TOUCHPAD_MAX as i64, TouchPhase::Click)
                .await?;
        }
        Ok(())
    }

    pub async fn raw(&self, identifier: &str, content: Value) -> Result<Value> {
        self.request(identifier, content).await
    }
}

fn public_id(credentials: &Credentials) -> String {
    let mut bytes = credentials.device_public_key[..6].to_vec();
    bytes[0] = 0xFF;
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

pub fn dict(pairs: Vec<(&str, Value)>) -> Value {
    Value::Dict(
        pairs
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect(),
    )
}

async fn reader_loop(
    mut reader: OwnedReadHalf,
    mut buffer: Vec<u8>,
    mut cipher: Option<Direction>,
    pending: Arc<Mutex<HashMap<u16, oneshot::Sender<Value>>>>,
    events: broadcast::Sender<Event>,
    alive: Arc<AtomicBool>,
) {
    loop {
        let (frame_type, payload) = match read_frame(&mut reader, &mut buffer, &mut cipher).await {
            Ok(frame) => frame,
            Err(_) => break,
        };
        if !frame_type.is_opack() {
            continue;
        }
        let Ok((value, _)) = opack::unpack(&payload) else {
            continue;
        };
        match value.get("_t").and_then(Value::as_i64) {
            Some(1) => {
                let name = value
                    .get("_i")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let content = value.get("_c").cloned().unwrap_or(Value::Null);
                let _ = events.send(Event { name, content });
            }
            Some(3) => {
                if let Some(xid) = value.get("_x").and_then(Value::as_i64) {
                    if let Some(sender) = pending.lock().unwrap().remove(&(xid as u16)) {
                        let _ = sender.send(value);
                    }
                }
            }
            _ => {}
        }
    }
    alive.store(false, Ordering::SeqCst);
    pending.lock().unwrap().clear();
}

async fn read_frame(
    reader: &mut OwnedReadHalf,
    buffer: &mut Vec<u8>,
    cipher: &mut Option<Direction>,
) -> Result<(FrameType, Vec<u8>)> {
    loop {
        if buffer.len() >= HEADER_LEN {
            let header: [u8; HEADER_LEN] = buffer[..HEADER_LEN].try_into().unwrap();
            let len = payload_len(&header);
            if buffer.len() >= HEADER_LEN + len {
                let payload = buffer[HEADER_LEN..HEADER_LEN + len].to_vec();
                buffer.drain(..HEADER_LEN + len);
                let frame_type = FrameType::from_code(header[0]);
                let payload = match (cipher.as_mut(), len) {
                    (Some(cipher), len) if len > 0 => cipher.open(&payload, &header)?,
                    _ => payload,
                };
                return Ok((frame_type, payload));
            }
        }
        let mut chunk = [0u8; 8192];
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            return Err(Error::Protocol("the Apple TV closed the connection".into()));
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
}
