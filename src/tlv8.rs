//! TLV8 encoding used by the HAP pairing handshake.

#[derive(Debug, Default, Clone)]
pub struct Tlv(Vec<(u8, Vec<u8>)>);

pub mod tag {
    pub const METHOD: u8 = 0x00;
    pub const IDENTIFIER: u8 = 0x01;
    pub const SALT: u8 = 0x02;
    pub const PUBLIC_KEY: u8 = 0x03;
    pub const PROOF: u8 = 0x04;
    pub const ENCRYPTED_DATA: u8 = 0x05;
    pub const SEQ_NO: u8 = 0x06;
    pub const ERROR: u8 = 0x07;
    pub const BACKOFF: u8 = 0x08;
    pub const SIGNATURE: u8 = 0x0A;
    pub const NAME: u8 = 0x11;
}

impl Tlv {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn push(mut self, tag: u8, value: impl Into<Vec<u8>>) -> Self {
        self.0.push((tag, value.into()));
        self
    }

    pub fn get(&self, tag: u8) -> Option<&[u8]> {
        self.0
            .iter()
            .find(|(t, _)| *t == tag)
            .map(|(_, v)| v.as_slice())
    }

    pub fn require(&self, tag: u8, what: &str) -> Result<&[u8], super::Error> {
        self.get(tag)
            .ok_or_else(|| super::Error::Protocol(format!("pairing response is missing {what}")))
    }

    pub fn error_code(&self) -> Option<u8> {
        self.get(tag::ERROR).and_then(|v| v.first().copied())
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (tag, value) in &self.0 {
            if value.is_empty() {
                out.extend_from_slice(&[*tag, 0]);
                continue;
            }
            for chunk in value.chunks(255) {
                out.push(*tag);
                out.push(chunk.len() as u8);
                out.extend_from_slice(chunk);
            }
        }
        out
    }

    pub fn decode(data: &[u8]) -> Result<Tlv, super::Error> {
        let mut items: Vec<(u8, Vec<u8>)> = Vec::new();
        let mut pos = 0;
        while pos < data.len() {
            if pos + 2 > data.len() {
                return Err(super::Error::Protocol("truncated TLV".into()));
            }
            let tag = data[pos];
            let len = data[pos + 1] as usize;
            let start = pos + 2;
            let end = start + len;
            if end > data.len() {
                return Err(super::Error::Protocol("truncated TLV value".into()));
            }
            match items.iter_mut().find(|(t, _)| *t == tag) {
                Some((_, existing)) => existing.extend_from_slice(&data[start..end]),
                None => items.push((tag, data[start..end].to_vec())),
            }
            pos = end;
        }
        Ok(Tlv(items))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_and_merges_continuations() {
        let tlv = Tlv::new()
            .push(tag::SEQ_NO, vec![1])
            .push(tag::PUBLIC_KEY, vec![7u8; 300]);
        let encoded = tlv.encode();
        let decoded = Tlv::decode(&encoded).unwrap();
        assert_eq!(decoded.get(tag::SEQ_NO), Some([1u8].as_slice()));
        assert_eq!(decoded.get(tag::PUBLIC_KEY).unwrap().len(), 300);
    }

    #[test]
    fn keeps_empty_values() {
        let decoded = Tlv::decode(&[tag::ERROR, 0]).unwrap();
        assert_eq!(decoded.get(tag::ERROR), Some([].as_slice()));
        assert_eq!(decoded.error_code(), None);
    }
}
