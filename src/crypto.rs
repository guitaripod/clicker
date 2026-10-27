//! ChaCha20-Poly1305 framing and HKDF-SHA512 key derivation.

use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce};
use hkdf::Hkdf;
use sha2::Sha512;

const NONCE_LEN: usize = 12;

/// One direction of an encrypted stream. Companion uses a little-endian counter
/// nonce, and the counter only advances when a frame carries a payload.
#[derive(Debug)]
pub struct Direction {
    key: [u8; 32],
    counter: u64,
}

impl Direction {
    pub fn new(key: [u8; 32]) -> Self {
        Self { key, counter: 0 }
    }

    pub fn seal(&mut self, plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>, super::Error> {
        let cipher = ChaCha20Poly1305::new((&self.key).into());
        let nonce = counter_nonce(self.counter);
        self.counter += 1;
        cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .map_err(|_| super::Error::Protocol("frame encryption failed".into()))
    }

    pub fn open(&mut self, ciphertext: &[u8], aad: &[u8]) -> Result<Vec<u8>, super::Error> {
        let cipher = ChaCha20Poly1305::new((&self.key).into());
        let nonce = counter_nonce(self.counter);
        self.counter += 1;
        cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: ciphertext,
                    aad,
                },
            )
            .map_err(|_| super::Error::Authentication("frame decryption failed".into()))
    }
}

fn counter_nonce(counter: u64) -> Nonce {
    let mut nonce = [0u8; NONCE_LEN];
    nonce[..8].copy_from_slice(&counter.to_le_bytes());
    Nonce::from(nonce)
}

/// Pairing TLVs are encrypted with fixed ASCII nonces such as `PS-Msg05`, which
/// sit in the low eight nonce bytes.
pub fn seal_with_label(
    key: &[u8; 32],
    label: &str,
    plaintext: &[u8],
) -> Result<Vec<u8>, super::Error> {
    let cipher = ChaCha20Poly1305::new(key.into());
    let nonce = label_nonce(label)?;
    cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad: &[],
            },
        )
        .map_err(|_| super::Error::Protocol("pairing encryption failed".into()))
}

pub fn open_with_label(
    key: &[u8; 32],
    label: &str,
    ciphertext: &[u8],
) -> Result<Vec<u8>, super::Error> {
    let cipher = ChaCha20Poly1305::new(key.into());
    let nonce = label_nonce(label)?;
    cipher
        .decrypt(
            &nonce,
            Payload {
                msg: ciphertext,
                aad: &[],
            },
        )
        .map_err(|_| super::Error::Authentication("pairing decryption failed".into()))
}

fn label_nonce(label: &str) -> Result<Nonce, super::Error> {
    let bytes = label.as_bytes();
    if bytes.len() > NONCE_LEN - 4 {
        return Err(super::Error::Protocol(format!(
            "nonce label too long: {label}"
        )));
    }
    let mut nonce = [0u8; NONCE_LEN];
    nonce[NONCE_LEN - bytes.len()..].copy_from_slice(bytes);
    Ok(Nonce::from(nonce))
}

pub fn hkdf_sha512(salt: &[u8], info: &str, input: &[u8]) -> [u8; 32] {
    let hkdf = Hkdf::<Sha512>::new(Some(salt), input);
    let mut output = [0u8; 32];
    hkdf.expand(info.as_bytes(), &mut output)
        .expect("32 bytes is a valid HKDF-SHA512 output length");
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_nonces_are_twelve_byte_little_endian_counters() {
        assert_eq!(
            counter_nonce(1).as_slice(),
            &[1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(
            counter_nonce(0x0102).as_slice(),
            &[2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn directions_roundtrip_between_peers() {
        let mut client_out = Direction::new([1; 32]);
        let mut server_in = Direction::new([1; 32]);
        for i in 0..3u8 {
            let sealed = client_out.seal(&[i; 8], b"head").unwrap();
            assert_eq!(server_in.open(&sealed, b"head").unwrap(), vec![i; 8]);
        }
        assert!(server_in.open(&[0u8; 24], b"head").is_err());
    }

    #[test]
    fn label_nonce_pins_the_hap_names() {
        let key = [9u8; 32];
        let sealed = seal_with_label(&key, "PS-Msg05", b"hello").unwrap();
        assert_eq!(
            open_with_label(&key, "PS-Msg05", &sealed).unwrap(),
            b"hello"
        );
        assert!(open_with_label(&key, "PS-Msg06", &sealed).is_err());
    }
}
