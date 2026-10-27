//! HAP pair-setup (PIN pairing) and pair-verify (session key agreement).

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use rand::RngCore;

use crate::companion::conn::Connection;
use crate::companion::frame::FrameType;
use crate::crypto::{hkdf_sha512, open_with_label, seal_with_label};
use crate::opack::{self, Value};
use crate::srp::SrpClient;
use crate::tlv8::{tag, Tlv};
use crate::{Error, Result};

pub const PAIRING_DATA: &str = "_pd";

const SIGN_SALT: &str = "Pair-Setup-Controller-Sign-Salt";
const SIGN_INFO: &str = "Pair-Setup-Controller-Sign-Info";
const ACCESSORY_SIGN_SALT: &str = "Pair-Setup-Accessory-Sign-Salt";
const ACCESSORY_SIGN_INFO: &str = "Pair-Setup-Accessory-Sign-Info";
const SETUP_SALT: &str = "Pair-Setup-Encrypt-Salt";
const SETUP_INFO: &str = "Pair-Setup-Encrypt-Info";
const VERIFY_SALT: &str = "Pair-Verify-Encrypt-Salt";
const VERIFY_INFO: &str = "Pair-Verify-Encrypt-Info";

/// Credentials produced by pairing, in the same order and encoding pyatv uses so
/// existing setups can be imported.
#[derive(Debug, Clone)]
pub struct Credentials {
    /// The Apple TV's long-term Ed25519 public key.
    pub device_public_key: [u8; 32],
    /// Our long-term Ed25519 secret key.
    pub client_secret_key: [u8; 32],
    /// The Apple TV's pairing identifier.
    pub device_id: Vec<u8>,
    /// Our pairing identifier (a UUID string, as bytes).
    pub client_id: Vec<u8>,
}

impl Credentials {
    pub fn to_pairing_string(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            hex(&self.device_public_key),
            hex(&self.client_secret_key),
            hex(&self.device_id),
            hex(&self.client_id)
        )
    }

    pub fn from_pairing_string(value: &str) -> Result<Self> {
        let parts: Vec<&str> = value.split(':').filter(|p| !p.is_empty()).collect();
        if parts.len() != 4 {
            return Err(Error::Config(
                "credential string must have four fields".into(),
            ));
        }
        let key: [u8; 32] = unhex(parts[0])?
            .try_into()
            .map_err(|_| Error::Config("device public key must be 32 bytes".into()))?;
        let secret: [u8; 32] = unhex(parts[1])?
            .try_into()
            .map_err(|_| Error::Config("client secret key must be 32 bytes".into()))?;
        Ok(Self {
            device_public_key: key,
            client_secret_key: secret,
            device_id: unhex(parts[2])?,
            client_id: unhex(parts[3])?,
        })
    }
}

/// A pair-setup exchange in two phases, because the Apple TV only displays its
/// PIN after the first message has been sent.
pub struct PairSetup {
    salt: Vec<u8>,
    server_public: Vec<u8>,
}

impl PairSetup {
    /// M1: start pairing. The device shows a PIN once this returns.
    pub async fn begin(conn: &mut Connection) -> Result<Self> {
        let start = Tlv::new()
            .push(tag::METHOD, vec![0x00])
            .push(tag::SEQ_NO, vec![0x01]);
        let response = conn
            .exchange_auth(
                FrameType::PsStart,
                FrameType::PsNext,
                Value::Dict(vec![
                    (PAIRING_DATA.into(), Value::Bytes(start.encode())),
                    ("_pwTy".into(), Value::Int(1)),
                ]),
            )
            .await?;
        let tlv = pairing_tlv(&response)?;
        crate::log::info(format_args!(
            "pair-setup started; the Apple TV is showing a PIN"
        ));
        Ok(Self {
            salt: tlv.require(tag::SALT, "salt")?.to_vec(),
            server_public: tlv.require(tag::PUBLIC_KEY, "public key")?.to_vec(),
        })
    }

    /// M3 and M5: prove knowledge of the PIN and store the exchanged keys.
    pub async fn finish(
        self,
        conn: &mut Connection,
        display_name: &str,
        pin: &str,
    ) -> Result<Credentials> {
        let mut srp = SrpClient::new(pin);
        let client_public = srp.public();
        let proof = srp.process(&self.salt, &self.server_public)?;

        let m3 = Tlv::new()
            .push(tag::SEQ_NO, vec![0x03])
            .push(tag::PUBLIC_KEY, client_public)
            .push(tag::PROOF, proof);
        let response = conn
            .exchange_auth(
                FrameType::PsNext,
                FrameType::PsNext,
                Value::Dict(vec![
                    (PAIRING_DATA.into(), Value::Bytes(m3.encode())),
                    ("_pwTy".into(), Value::Int(1)),
                ]),
            )
            .await?;
        let tlv = pairing_tlv(&response)?;
        srp.verify_server(tlv.require(tag::PROOF, "proof")?)?;

        let mut secret = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut secret);
        let signing_key = SigningKey::from_bytes(&secret);
        let client_pairing_id = uuid_string().into_bytes();

        let x = hkdf_sha512(SIGN_SALT.as_bytes(), SIGN_INFO, srp.session_key());
        let session_key = hkdf_sha512(SETUP_SALT.as_bytes(), SETUP_INFO, srp.session_key());

        let mut device_info = x.to_vec();
        device_info.extend_from_slice(&client_pairing_id);
        device_info.extend_from_slice(signing_key.verifying_key().as_bytes());
        let signature = signing_key.sign(&device_info);

        let name = opack::pack(&Value::Dict(vec![(
            "name".into(),
            Value::Str(display_name.to_string()),
        )]))?;

        let m5 = Tlv::new()
            .push(tag::IDENTIFIER, client_pairing_id.clone())
            .push(
                tag::PUBLIC_KEY,
                signing_key.verifying_key().as_bytes().to_vec(),
            )
            .push(tag::SIGNATURE, signature.to_bytes().to_vec())
            .push(tag::NAME, name)
            .encode();
        let sealed = seal_with_label(&session_key, "PS-Msg05", &m5)?;

        let response = conn
            .exchange_auth(
                FrameType::PsNext,
                FrameType::PsNext,
                Value::Dict(vec![
                    (
                        PAIRING_DATA.into(),
                        Value::Bytes(
                            Tlv::new()
                                .push(tag::SEQ_NO, vec![0x05])
                                .push(tag::ENCRYPTED_DATA, sealed)
                                .encode(),
                        ),
                    ),
                    ("_pwTy".into(), Value::Int(1)),
                ]),
            )
            .await?;
        let tlv = pairing_tlv(&response)?;
        let decrypted = open_with_label(
            &session_key,
            "PS-Msg06",
            tlv.require(tag::ENCRYPTED_DATA, "encrypted data")?,
        )?;
        let m6 = Tlv::decode(&decrypted)?;
        let device_id = m6.require(tag::IDENTIFIER, "identifier")?.to_vec();
        let device_key: [u8; 32] = m6
            .require(tag::PUBLIC_KEY, "public key")?
            .try_into()
            .map_err(|_| Error::Authentication("device public key is not 32 bytes".into()))?;
        let device_signature: [u8; 64] = m6
            .require(tag::SIGNATURE, "signature")?
            .try_into()
            .map_err(|_| Error::Authentication("device signature is not 64 bytes".into()))?;

        // The device signs with its own HKDF context, not the controller's.
        let accessory_x = hkdf_sha512(
            ACCESSORY_SIGN_SALT.as_bytes(),
            ACCESSORY_SIGN_INFO,
            srp.session_key(),
        );
        let mut signed = accessory_x.to_vec();
        signed.extend_from_slice(&device_id);
        signed.extend_from_slice(&device_key);
        VerifyingKey::from_bytes(&device_key)
            .map_err(|_| Error::Authentication("device public key is not valid".into()))?
            .verify_strict(&signed, &Signature::from_bytes(&device_signature))
            .map_err(|_| Error::Authentication("device signature did not verify".into()))?;

        crate::log::info(format_args!(
            "pair-setup finished; device signature verified"
        ));
        Ok(Credentials {
            device_public_key: device_key,
            client_secret_key: signing_key.to_bytes(),
            device_id,
            client_id: client_pairing_id,
        })
    }
}

/// Prove we already hold the device's credentials and derive session keys.
/// Returns `(outbound, inbound)`.
pub async fn verify(
    conn: &mut Connection,
    credentials: &Credentials,
) -> Result<([u8; 32], [u8; 32])> {
    let secret = x25519_dalek::StaticSecret::random_from_rng(rand::rngs::OsRng);
    let public = x25519_dalek::PublicKey::from(&secret);

    let m1 = Tlv::new()
        .push(tag::SEQ_NO, vec![0x01])
        .push(tag::PUBLIC_KEY, public.as_bytes().to_vec());
    let response = conn
        .exchange_auth(
            FrameType::PvStart,
            FrameType::PvNext,
            Value::Dict(vec![
                (PAIRING_DATA.into(), Value::Bytes(m1.encode())),
                ("_auTy".into(), Value::Int(4)),
            ]),
        )
        .await?;
    let tlv = pairing_tlv(&response)?;
    let server_public: [u8; 32] = tlv
        .require(tag::PUBLIC_KEY, "public key")?
        .try_into()
        .map_err(|_| Error::Authentication("device ephemeral key is not 32 bytes".into()))?;
    let encrypted = tlv.require(tag::ENCRYPTED_DATA, "encrypted data")?;

    let shared = secret.diffie_hellman(&x25519_dalek::PublicKey::from(server_public));
    let session_key = hkdf_sha512(VERIFY_SALT.as_bytes(), VERIFY_INFO, shared.as_bytes());

    let decrypted = open_with_label(&session_key, "PV-Msg02", encrypted)?;
    let m2 = Tlv::decode(&decrypted)?;
    let device_id = m2.require(tag::IDENTIFIER, "identifier")?;
    if device_id != credentials.device_id.as_slice() {
        return Err(Error::Authentication(
            "the device identity does not match the stored credentials".into(),
        ));
    }
    let signature: [u8; 64] = m2
        .require(tag::SIGNATURE, "signature")?
        .try_into()
        .map_err(|_| Error::Authentication("device signature is not 64 bytes".into()))?;

    let mut signed = server_public.to_vec();
    signed.extend_from_slice(device_id);
    signed.extend_from_slice(public.as_bytes());
    VerifyingKey::from_bytes(&credentials.device_public_key)
        .map_err(|_| Error::Authentication("stored device key is invalid".into()))?
        .verify_strict(&signed, &Signature::from_bytes(&signature))
        .map_err(|_| {
            Error::Authentication("the device failed to authenticate its session".into())
        })?;

    let signing_key = SigningKey::from_bytes(&credentials.client_secret_key);
    let mut client_signed = public.as_bytes().to_vec();
    client_signed.extend_from_slice(&credentials.client_id);
    client_signed.extend_from_slice(&server_public);
    let client_signature = signing_key.sign(&client_signed);

    let m3 = Tlv::new()
        .push(tag::SEQ_NO, vec![0x03])
        .push(tag::IDENTIFIER, credentials.client_id.clone())
        .push(tag::SIGNATURE, client_signature.to_bytes().to_vec())
        .encode();
    let sealed = seal_with_label(&session_key, "PV-Msg03", &m3)?;

    let response = conn
        .exchange_auth(
            FrameType::PvNext,
            FrameType::PvNext,
            Value::Dict(vec![(
                PAIRING_DATA.into(),
                Value::Bytes(
                    Tlv::new()
                        .push(tag::SEQ_NO, vec![0x03])
                        .push(tag::ENCRYPTED_DATA, sealed)
                        .encode(),
                ),
            )]),
        )
        .await?;
    pairing_tlv(&response)?;

    crate::log::debug(format_args!("pair-verify finished"));
    Ok((
        hkdf_sha512(b"", "ClientEncrypt-main", shared.as_bytes()),
        hkdf_sha512(b"", "ServerEncrypt-main", shared.as_bytes()),
    ))
}

fn pairing_tlv(response: &Value) -> Result<Tlv> {
    let data = response
        .get(PAIRING_DATA)
        .and_then(|value| match value {
            Value::Bytes(bytes) => Some(bytes.clone()),
            _ => None,
        })
        .ok_or_else(|| Error::Protocol("pairing response has no pairing data".into()))?;
    let tlv = Tlv::decode(&data)?;
    if let Some(code) = tlv.error_code() {
        if code != 0 {
            return Err(pairing_error(code, &tlv));
        }
    }
    Ok(tlv)
}

/// HAP error codes: 0x02 is a wrong PIN, 0x03 is a rate limit, the rest are
/// protocol-level refusals worth naming.
fn pairing_error(code: u8, tlv: &Tlv) -> Error {
    match code {
        0x02 => Error::Authentication("the code did not match".into()),
        0x03 => Error::PairingBackoff {
            seconds: tlv.get(tag::BACKOFF).map(backoff_seconds),
        },
        0x01 => Error::Protocol("the Apple TV could not start pairing".into()),
        0x04 => Error::Protocol("the Apple TV is already paired with too many remotes".into()),
        0x05 => Error::Authentication("too many incorrect attempts".into()),
        0x06 => Error::Protocol("the Apple TV is not available for pairing".into()),
        0x07 => Error::Protocol("the Apple TV is busy; try again in a moment".into()),
        other => Error::Protocol(format!(
            "the Apple TV refused pairing (error 0x{other:02x})"
        )),
    }
}

fn backoff_seconds(value: &[u8]) -> u64 {
    value
        .iter()
        .rev()
        .fold(0u64, |total, byte| total.saturating_mul(256) + *byte as u64)
}

fn uuid_string() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0F) | 0x40;
    bytes[8] = (bytes[8] & 0x3F) | 0x80;
    let h: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(value: &str) -> Result<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return Err(Error::Config("hex string has an odd length".into()));
    }
    (0..value.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&value[i..i + 2], 16)
                .map_err(|_| Error::Config(format!("invalid hex: {value}")))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_is_a_little_endian_integer() {
        assert_eq!(backoff_seconds(&[2]), 2);
        assert_eq!(backoff_seconds(&[0x2c, 0x01]), 300);
        assert_eq!(backoff_seconds(&[]), 0);
    }

    #[test]
    fn hap_error_codes_map_to_actions() {
        assert!(matches!(
            pairing_error(0x02, &Tlv::default()),
            Error::Authentication(_)
        ));
        assert!(matches!(
            pairing_error(0x03, &Tlv::new().push(tag::BACKOFF, vec![30])),
            Error::PairingBackoff { seconds: Some(30) }
        ));
        assert!(matches!(
            pairing_error(0x07, &Tlv::default()),
            Error::Protocol(_)
        ));
    }
}
