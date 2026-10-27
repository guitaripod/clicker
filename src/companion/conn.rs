//! The Companion transport: TCP, 4-byte framed, optionally encrypted.

use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;

use super::frame::{header, payload_len, FrameType, AUTH_TAG_LEN, HEADER_LEN};
use crate::crypto::Direction;
use crate::opack::{self, Value};
use crate::{Error, Result};

pub struct Connection {
    stream: TcpStream,
    buffer: Vec<u8>,
    out: Option<Direction>,
    in_cipher: Option<Direction>,
    next_xid: u16,
    timeout: Duration,
}

impl Connection {
    pub async fn open(address: &str, port: u16, timeout: Duration) -> Result<Self> {
        let stream = tokio::time::timeout(timeout, TcpStream::connect((address, port)))
            .await
            .map_err(|_| Error::Timeout(timeout))?
            .map_err(|err| Error::Protocol(format!("cannot reach {address}:{port}: {err}")))?;
        stream.set_nodelay(true).ok();
        Ok(Self {
            stream,
            buffer: Vec::with_capacity(1024),
            out: None,
            in_cipher: None,
            next_xid: rand::random::<u16>(),
            timeout,
        })
    }

    pub fn enable_encryption(&mut self, out: [u8; 32], input: [u8; 32]) {
        self.out = Some(Direction::new(out));
        self.in_cipher = Some(Direction::new(input));
    }

    pub async fn send(&mut self, frame_type: FrameType, payload: &[u8]) -> Result<()> {
        let encrypted = self.out.is_some() && !payload.is_empty();
        let wire_len = payload.len() + if encrypted { AUTH_TAG_LEN } else { 0 };
        let header = header(frame_type, wire_len);
        let body = match (&mut self.out, encrypted) {
            (Some(cipher), true) => cipher.seal(payload, &header)?,
            _ => payload.to_vec(),
        };
        crate::log::debug(format_args!(
            "send {:?} frame, {} bytes, encrypted={encrypted}",
            frame_type,
            payload.len()
        ));
        self.stream.write_all(&header).await?;
        self.stream.write_all(&body).await?;
        self.stream.flush().await?;
        Ok(())
    }

    pub async fn receive(&mut self) -> Result<(FrameType, Vec<u8>)> {
        let timeout = self.timeout;
        tokio::time::timeout(timeout, self.receive_inner())
            .await
            .map_err(|_| Error::Timeout(timeout))?
    }

    async fn receive_inner(&mut self) -> Result<(FrameType, Vec<u8>)> {
        loop {
            if self.buffer.len() >= HEADER_LEN {
                let header: [u8; HEADER_LEN] = self.buffer[..HEADER_LEN].try_into().unwrap();
                let len = payload_len(&header);
                if self.buffer.len() >= HEADER_LEN + len {
                    let payload = self.buffer[HEADER_LEN..HEADER_LEN + len].to_vec();
                    self.buffer.drain(..HEADER_LEN + len);
                    let frame_type = FrameType::from_code(header[0]);
                    let payload = match (&mut self.in_cipher, len) {
                        (Some(cipher), len) if len > 0 => cipher.open(&payload, &header)?,
                        _ => payload,
                    };
                    crate::log::debug(format_args!(
                        "recv {:?} frame, {} bytes",
                        frame_type,
                        payload.len()
                    ));
                    return Ok((frame_type, payload));
                }
            }

            let mut chunk = [0u8; 8192];
            let read = self.stream.read(&mut chunk).await?;
            if read == 0 {
                return Err(Error::Protocol("the Apple TV closed the connection".into()));
            }
            self.buffer.extend_from_slice(&chunk[..read]);
        }
    }

    /// Send a pairing message and wait for the reply frame of `expected` type.
    /// Authentication frames are dispatched by frame type, not by XID.
    pub async fn exchange_auth(
        &mut self,
        frame_type: FrameType,
        expected: FrameType,
        data: Value,
    ) -> Result<Value> {
        let mut data = data;
        if let Value::Dict(pairs) = &mut data {
            if !pairs.iter().any(|(key, _)| key == "_x") {
                pairs.push(("_x".into(), Value::Int(self.next_xid as i64)));
                self.next_xid = self.next_xid.wrapping_add(1);
            }
        }
        let packed = opack::pack(&data)?;
        self.send(frame_type, &packed).await?;

        loop {
            let (received_type, payload) = self.receive().await?;
            if received_type != expected {
                continue;
            }
            let (value, _) = opack::unpack(&payload)?;
            return Ok(value);
        }
    }

    /// Hand the socket to the background reader and writer tasks.
    pub fn split(
        self,
    ) -> (
        OwnedReadHalf,
        OwnedWriteHalf,
        Vec<u8>,
        Option<Direction>,
        Option<Direction>,
    ) {
        let (reader, writer) = self.stream.into_split();
        (reader, writer, self.buffer, self.out, self.in_cipher)
    }
}

/// Frames written by the background writer task.
pub struct Outbound {
    pub frame_type: FrameType,
    pub payload: Vec<u8>,
}

pub async fn writer_loop(
    mut writer: OwnedWriteHalf,
    mut cipher: Option<Direction>,
    mut rx: tokio::sync::mpsc::Receiver<Outbound>,
) {
    while let Some(frame) = rx.recv().await {
        let encrypted = cipher.is_some() && !frame.payload.is_empty();
        let wire_len = frame.payload.len() + if encrypted { AUTH_TAG_LEN } else { 0 };
        let header = header(frame.frame_type, wire_len);
        let body = match (&mut cipher, encrypted) {
            (Some(cipher), true) => match cipher.seal(&frame.payload, &header) {
                Ok(body) => body,
                Err(_) => return,
            },
            _ => frame.payload,
        };
        if writer.write_all(&header).await.is_err() || writer.write_all(&body).await.is_err() {
            return;
        }
        if writer.flush().await.is_err() {
            return;
        }
    }
}
