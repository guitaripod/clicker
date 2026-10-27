//! OPACK serialization, the binary format Companion speaks.
//!
//! The format is a tagged binary encoding with an object list: any container or
//! string larger than a single byte may be emitted once and referenced by index
//! later in the same message. Apple uses those references heavily in replies, so
//! both directions implement the list.

use serde_json::json;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Uuid([u8; 16]),
    Str(String),
    Bytes(Vec<u8>),
    Array(Vec<Value>),
    Dict(Vec<(String, Value)>),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Dict(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Float(f) => Some(*f as i64),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            Value::Int(i) => Some(*i as f64),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<&Vec<(String, Value)>> {
        match self {
            Value::Dict(pairs) => Some(pairs),
            _ => None,
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Value::Null => serde_json::Value::Null,
            Value::Bool(b) => json!(b),
            Value::Int(i) => json!(i),
            Value::Float(f) => json!(f),
            Value::Uuid(bytes) => json!(format_uuid(bytes)),
            Value::Str(s) => json!(s),
            Value::Bytes(b) => json!(hex(b)),
            Value::Array(items) => {
                serde_json::Value::Array(items.iter().map(|i| i.to_json()).collect())
            }
            Value::Dict(pairs) => {
                let mut map = serde_json::Map::new();
                for (k, v) in pairs {
                    map.insert(k.clone(), v.to_json());
                }
                serde_json::Value::Object(map)
            }
        }
    }

    pub fn from_json(value: &serde_json::Value) -> Result<Value, Error> {
        Ok(match value {
            serde_json::Value::Null => Value::Null,
            serde_json::Value::Bool(b) => Value::Bool(*b),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Value::Int(i)
                } else if let Some(f) = n.as_f64() {
                    Value::Float(f)
                } else {
                    return Err(Error::Unsupported("number out of range".into()));
                }
            }
            serde_json::Value::String(s) => Value::Str(s.clone()),
            serde_json::Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(Value::from_json)
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            serde_json::Value::Object(map) => Value::Dict(
                map.iter()
                    .map(|(k, v)| Ok((k.clone(), Value::from_json(v)?)))
                    .collect::<Result<Vec<_>, Error>>()?,
            ),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("truncated OPACK data")]
    Truncated,
    #[error("unsupported OPACK type 0x{0:02x}")]
    UnknownType(u8),
    #[error("OPACK object reference out of range")]
    BadReference,
    #[error("OPACK dict keys must be strings")]
    NonStringKey,
    #[error("{0}")]
    Unsupported(String),
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn format_uuid(bytes: &[u8; 16]) -> String {
    let h = hex(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_json())
    }
}

pub fn pack(value: &Value) -> Result<Vec<u8>, Error> {
    let mut objects = Vec::new();
    pack_value(value, &mut objects)
}

fn pack_value(value: &Value, objects: &mut Vec<Vec<u8>>) -> Result<Vec<u8>, Error> {
    let mut out = match value {
        Value::Null => vec![0x04],
        Value::Bool(true) => vec![0x01],
        Value::Bool(false) => vec![0x02],
        Value::Uuid(bytes) => {
            let mut buf = vec![0x05];
            buf.extend_from_slice(bytes);
            buf
        }
        Value::Int(v) => pack_int(*v)?,
        Value::Float(v) => {
            let mut buf = vec![0x36];
            buf.extend_from_slice(&v.to_le_bytes());
            buf
        }
        Value::Str(s) => pack_str(s.as_bytes()),
        Value::Bytes(b) => pack_data(b),
        Value::Array(items) => {
            let mut buf = vec![0xD0 | (items.len().min(0xF) as u8)];
            for item in items {
                buf.extend_from_slice(&pack_value(item, objects)?);
            }
            if items.len() >= 0xF {
                buf.push(0x03);
            }
            buf
        }
        Value::Dict(pairs) => {
            let mut buf = vec![0xE0 | (pairs.len().min(0xF) as u8)];
            for (key, item) in pairs {
                buf.extend_from_slice(&pack_value(&Value::Str(key.clone()), objects)?);
                buf.extend_from_slice(&pack_value(item, objects)?);
            }
            if pairs.len() >= 0xF {
                buf.push(0x03);
            }
            buf
        }
    };

    if out.len() > 1 {
        if let Some(index) = objects.iter().position(|existing| *existing == out) {
            out = pack_reference(index);
        } else {
            objects.push(out.clone());
        }
    }

    Ok(out)
}

fn pack_reference(index: usize) -> Vec<u8> {
    if index < 0x21 {
        vec![0xA0 + index as u8]
    } else if index <= 0xFF {
        vec![0xC1, index as u8]
    } else if index <= 0xFFFF {
        let mut buf = vec![0xC2];
        buf.extend_from_slice(&(index as u16).to_le_bytes());
        buf
    } else {
        let mut buf = vec![0xC3];
        buf.extend_from_slice(&(index as u32).to_le_bytes());
        buf
    }
}

fn pack_int(value: i64) -> Result<Vec<u8>, Error> {
    if value < 0 {
        return Err(Error::Unsupported(format!(
            "negative int {value} is not encodable as OPACK int"
        )));
    }
    let value = value as u64;
    let mut buf = Vec::with_capacity(9);
    if value < 0x28 {
        buf.push(0x08 + value as u8);
    } else if value <= 0xFF {
        buf.push(0x30);
        buf.push(value as u8);
    } else if value <= 0xFFFF {
        buf.push(0x31);
        buf.extend_from_slice(&(value as u16).to_le_bytes());
    } else if value <= 0xFFFF_FFFF {
        buf.push(0x32);
        buf.extend_from_slice(&(value as u32).to_le_bytes());
    } else {
        buf.push(0x33);
        buf.extend_from_slice(&value.to_le_bytes());
    }
    Ok(buf)
}

fn pack_str(data: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(data.len() + 5);
    let len = data.len();
    if len <= 0x20 {
        buf.push(0x40 + len as u8);
    } else if len <= 0xFF {
        buf.push(0x61);
        buf.push(len as u8);
    } else if len <= 0xFFFF {
        buf.push(0x62);
        buf.extend_from_slice(&(len as u16).to_le_bytes());
    } else if len <= 0xFF_FFFF {
        buf.push(0x63);
        buf.extend_from_slice(&(len as u32).to_le_bytes()[0..3]);
    } else {
        buf.push(0x64);
        buf.extend_from_slice(&(len as u32).to_le_bytes());
    }
    buf.extend_from_slice(data);
    buf
}

fn pack_data(data: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(data.len() + 9);
    let len = data.len();
    if len <= 0x20 {
        buf.push(0x70 + len as u8);
    } else if len <= 0xFF {
        buf.push(0x91);
        buf.push(len as u8);
    } else if len <= 0xFFFF {
        buf.push(0x92);
        buf.extend_from_slice(&(len as u16).to_le_bytes());
    } else if len <= 0xFFFF_FFFF {
        buf.push(0x93);
        buf.extend_from_slice(&(len as u32).to_le_bytes());
    } else {
        buf.push(0x94);
        buf.extend_from_slice(&(len as u64).to_le_bytes());
    }
    buf.extend_from_slice(data);
    buf
}

pub fn unpack(data: &[u8]) -> Result<(Value, &[u8]), Error> {
    let mut objects = Vec::new();
    unpack_value(data, &mut objects)
}

fn unpack_value<'a>(data: &'a [u8], objects: &mut Vec<Value>) -> Result<(Value, &'a [u8]), Error> {
    let tag = *data.first().ok_or(Error::Truncated)?;
    let rest = &data[1..];

    let (value, rest) = match tag {
        0x01 => (Value::Bool(true), rest),
        0x02 => (Value::Bool(false), rest),
        0x04 => (Value::Null, rest),
        0x03 => (Value::Null, rest),
        0x05 => {
            if rest.len() < 16 {
                return Err(Error::Truncated);
            }
            let mut uuid = [0u8; 16];
            uuid.copy_from_slice(&rest[..16]);
            (Value::Uuid(uuid), &rest[16..])
        }
        0x06 => {
            if rest.len() < 8 {
                return Err(Error::Truncated);
            }
            let mut raw = [0u8; 8];
            raw.copy_from_slice(&rest[..8]);
            (Value::Int(u64::from_le_bytes(raw) as i64), &rest[8..])
        }
        0x08..=0x2F => (Value::Int((tag - 0x08) as i64), rest),
        0x35 => {
            let arr: [u8; 4] = rest.get(..4).ok_or(Error::Truncated)?.try_into().unwrap();
            (Value::Float(f32::from_le_bytes(arr) as f64), &rest[4..])
        }
        0x36 => {
            let arr: [u8; 8] = rest.get(..8).ok_or(Error::Truncated)?.try_into().unwrap();
            (Value::Float(f64::from_le_bytes(arr)), &rest[8..])
        }
        0x30..=0x33 => {
            let size = 1usize << (tag & 0xF);
            let raw = rest.get(..size).ok_or(Error::Truncated)?;
            let mut value: u64 = 0;
            for (i, byte) in raw.iter().enumerate() {
                value |= (*byte as u64) << (8 * i);
            }
            (Value::Int(value as i64), &rest[size..])
        }
        0x40..=0x60 => {
            let len = (tag - 0x40) as usize;
            let raw = rest.get(..len).ok_or(Error::Truncated)?;
            (
                Value::Str(String::from_utf8_lossy(raw).into_owned()),
                &rest[len..],
            )
        }
        0x61..=0x64 => {
            let size = (tag & 0xF) as usize;
            let len = read_len(rest, size)?;
            let start = size;
            let raw = rest.get(start..start + len).ok_or(Error::Truncated)?;
            (
                Value::Str(String::from_utf8_lossy(raw).into_owned()),
                &rest[start + len..],
            )
        }
        0x70..=0x90 => {
            let len = (tag - 0x70) as usize;
            let raw = rest.get(..len).ok_or(Error::Truncated)?;
            (Value::Bytes(raw.to_vec()), &rest[len..])
        }
        0x91..=0x94 => {
            let size = 1usize << ((tag & 0xF) - 1);
            let len = read_len(rest, size)?;
            let raw = rest.get(size..size + len).ok_or(Error::Truncated)?;
            (Value::Bytes(raw.to_vec()), &rest[size + len..])
        }
        0xD0..=0xDF => unpack_array(tag & 0xF, rest, objects)?,
        0xE0..=0xEF => unpack_dict(tag & 0xF, rest, objects)?,
        0xA0..=0xBF => {
            let index = (tag - 0xA0) as usize;
            (
                objects.get(index).cloned().ok_or(Error::BadReference)?,
                rest,
            )
        }
        0xC1..=0xC4 => {
            let size = (tag & 0xF) as usize;
            let index = read_len(rest, size)?;
            (
                objects.get(index).cloned().ok_or(Error::BadReference)?,
                &rest[size..],
            )
        }
        other => return Err(Error::UnknownType(other)),
    };

    if tag != 0x03
        && !matches!(tag, 0x01 | 0x02 | 0x04)
        && !(0x08..=0x2F).contains(&tag)
        && !objects.contains(&value)
    {
        objects.push(value.clone());
    }

    Ok((value, rest))
}

fn read_len(data: &[u8], size: usize) -> Result<usize, Error> {
    let raw = data.get(..size).ok_or(Error::Truncated)?;
    let mut value = 0usize;
    for (i, byte) in raw.iter().enumerate() {
        value |= (*byte as usize) << (8 * i);
    }
    Ok(value)
}

fn unpack_array<'a>(
    count: u8,
    mut rest: &'a [u8],
    objects: &mut Vec<Value>,
) -> Result<(Value, &'a [u8]), Error> {
    let mut items = Vec::new();
    if count == 0xF {
        loop {
            match rest.first() {
                Some(0x03) => {
                    rest = &rest[1..];
                    break;
                }
                None => return Err(Error::Truncated),
                _ => {
                    let (item, next) = unpack_value(rest, objects)?;
                    items.push(item);
                    rest = next;
                }
            }
        }
    } else {
        for _ in 0..count {
            let (item, next) = unpack_value(rest, objects)?;
            items.push(item);
            rest = next;
        }
    }
    Ok((Value::Array(items), rest))
}

fn unpack_dict<'a>(
    count: u8,
    mut rest: &'a [u8],
    objects: &mut Vec<Value>,
) -> Result<(Value, &'a [u8]), Error> {
    let mut pairs: Vec<(String, Value)> = Vec::new();
    let mut read_pair = |rest: &mut &'a [u8]| -> Result<(), Error> {
        let (key, next) = unpack_value(rest, objects)?;
        let key = match key {
            Value::Str(s) => s,
            _ => return Err(Error::NonStringKey),
        };
        let (value, next) = unpack_value(next, objects)?;
        pairs.push((key, value));
        *rest = next;
        Ok(())
    };

    if count == 0xF {
        loop {
            match rest.first() {
                Some(0x03) => {
                    rest = &rest[1..];
                    break;
                }
                None => return Err(Error::Truncated),
                _ => read_pair(&mut rest)?,
            }
        }
    } else {
        for _ in 0..count {
            read_pair(&mut rest)?;
        }
    }
    Ok((Value::Dict(pairs), rest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn roundtrip(value: serde_json::Value) {
        let value = Value::from_json(&value).unwrap();
        let packed = pack(&value).unwrap();
        let (unpacked, rest) = unpack(&packed).unwrap();
        assert!(rest.is_empty());
        assert_eq!(unpacked, value, "packed {packed:02x?}");
    }

    #[test]
    fn roundtrips_scalars() {
        roundtrip(json!(null));
        roundtrip(json!(true));
        roundtrip(json!(false));
        roundtrip(json!(0));
        roundtrip(json!(27));
        roundtrip(json!(28));
        roundtrip(json!(255));
        roundtrip(json!(65535));
        roundtrip(json!(4294967295u32));
        roundtrip(json!(18446744073709551615u64));
        roundtrip(json!(1.5));
        roundtrip(json!(""));
        roundtrip(json!("_hidC"));
        roundtrip(json!("a".repeat(300)));
    }

    #[test]
    fn roundtrips_containers() {
        roundtrip(json!({"_i": "_hidC", "_t": 2, "_c": {"_hBtS": 1, "_hidC": 1}}));
        roundtrip(json!(["a", "b", 1, 2, {"k": "v"}]));
        let long: Vec<u32> = (0..20).collect();
        roundtrip(json!(long));
        let long_map: serde_json::Map<String, serde_json::Value> =
            (0..20).map(|i| (format!("k{i}"), json!(i))).collect();
        roundtrip(serde_json::Value::Object(long_map));
    }

    #[test]
    fn decodes_uuid_and_references() {
        let mut objects: Vec<Value> = Vec::new();
        let (value, _) = unpack_value(
            &[0x05, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
            &mut objects,
        )
        .unwrap();
        assert_eq!(
            value,
            Value::Uuid([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16])
        );
        assert_eq!(objects.len(), 1);

        let bytes = [
            0xE1, // dict, 1 pair
            0x43, b'k', b'e', b'y', 0x45, b'v', b'a', b'l', b'u', b'e',
        ];
        let (value, rest) = unpack(&bytes).unwrap();
        assert!(rest.is_empty());
        assert_eq!(value.get("key").unwrap().as_str(), Some("value"));

        let with_reference = [
            0xD2, // array of 2
            0xE1, 0x41, b'a', 0x41, b'b', // {"a": "b"} -> added to object list
            0xA2, // reference to it
        ];
        let (value, rest) = unpack(&with_reference).unwrap();
        assert!(rest.is_empty());
        assert_eq!(value.to_json(), json!([{ "a": "b" }, { "a": "b" }]));
    }
}
