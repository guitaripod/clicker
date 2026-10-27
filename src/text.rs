//! NSKeyedArchiver payloads for the Remote Text Input channel.
//!
//! The Apple TV expects the same archives the iOS keyboard extension sends, so
//! the object graphs below are fixed and pre-indexed exactly like pyatv's.

use plist::{Dictionary, Uid, Value};
use std::io::Cursor;

#[derive(Debug)]
pub struct TextInput {
    pub session_uuid: Vec<u8>,
    pub current_text: String,
}

/// Parse the `_tiD` archive returned by `_tiStart`.
pub fn parse_text_input(data: &[u8]) -> Result<TextInput, super::Error> {
    let root = Value::from_reader(Cursor::new(data)).map_err(|err| {
        super::Error::Protocol(format!("text input archive is not a plist: {err}"))
    })?;
    // `$top.sessionUUID` is a bare data object; older archives nest it inside
    // an NSUUID under `NS.uuidbytes`.
    let uuid = top_value(&root, "sessionUUID")
        .and_then(|value| value.as_data())
        .or_else(|| find(&root, "NS.uuidbytes").and_then(|value| value.as_data()))
        .ok_or_else(|| super::Error::Protocol("text input archive has no session UUID".into()))?;
    let text = find(&root, "contextBeforeInput")
        .and_then(|value| value.as_string())
        .unwrap_or_default();
    Ok(TextInput {
        session_uuid: uuid.to_vec(),
        current_text: text.to_string(),
    })
}

/// Read a key from `$top`, following `$objects` references.
fn top_value<'a>(root: &'a Value, key: &str) -> Option<&'a Value> {
    let top = root.as_dictionary()?.get("$top")?.as_dictionary()?;
    top.get(key).map(|value| deref(root, value))
}

/// Recursively search a keyed archive, following `$objects` references.
fn find<'a>(root: &'a Value, key: &str) -> Option<&'a Value> {
    find_at(root, root, key, 0)
}

fn find_at<'a>(root: &'a Value, value: &'a Value, key: &str, depth: usize) -> Option<&'a Value> {
    if depth > 8 {
        return None;
    }
    match value {
        Value::Dictionary(dict) => {
            if let Some(found) = dict.get(key) {
                return Some(deref(root, found));
            }
            dict.values()
                .find_map(|child| find_at(root, child, key, depth + 1))
        }
        Value::Array(items) => items
            .iter()
            .find_map(|child| find_at(root, child, key, depth + 1)),
        _ => None,
    }
}

fn deref<'a>(root: &'a Value, value: &'a Value) -> &'a Value {
    let Value::Uid(uid) = value else {
        return value;
    };
    let Some(Value::Array(objects)) = root.as_dictionary().and_then(|dict| dict.get("$objects"))
    else {
        return value;
    };
    objects
        .get(uid.get() as usize)
        .map(|object| deref(root, object))
        .unwrap_or(value)
}

/// Archive that clears the text field and asserts empty text.
pub fn clear_payload(session_uuid: &[u8]) -> Result<Vec<u8>, super::Error> {
    let objects = vec![
        Value::String("$null".into()),
        dict(vec![
            ("$class", uid(7)),
            ("targetSessionUUID", uid(5)),
            ("keyboardOutput", uid(2)),
            ("textToAssert", uid(4)),
        ]),
        dict(vec![("$class", uid(3))]),
        keyboard_output_class(),
        Value::String(String::new()),
        dict(vec![
            ("NS.uuidbytes", Value::Data(session_uuid.to_vec())),
            ("$class", uid(6)),
        ]),
        uuid_class(),
        text_operations_class(),
    ];
    archive(objects, "textOperations")
}

/// Archive that inserts `text` after the current insertion point.
pub fn input_payload(session_uuid: &[u8], text: &str) -> Result<Vec<u8>, super::Error> {
    let objects = vec![
        Value::String("$null".into()),
        dict(vec![
            ("keyboardOutput", uid(2)),
            ("$class", uid(7)),
            ("targetSessionUUID", uid(5)),
        ]),
        dict(vec![("insertionText", uid(3)), ("$class", uid(4))]),
        Value::String(text.to_string()),
        keyboard_output_class(),
        dict(vec![
            ("NS.uuidbytes", Value::Data(session_uuid.to_vec())),
            ("$class", uid(6)),
        ]),
        uuid_class(),
        text_operations_class(),
    ];
    archive(objects, "textOperations")
}

fn archive(objects: Vec<Value>, top_key: &str) -> Result<Vec<u8>, super::Error> {
    let mut top = Dictionary::new();
    top.insert(top_key.to_string(), uid(1));
    let mut root = Dictionary::new();
    root.insert("$version".into(), Value::Integer(100_000.into()));
    root.insert("$archiver".into(), Value::String("RTIKeyedArchiver".into()));
    root.insert("$top".into(), Value::Dictionary(top));
    root.insert("$objects".into(), Value::Array(objects));

    let mut out = Cursor::new(Vec::new());
    Value::Dictionary(root)
        .to_writer_binary(&mut out)
        .map_err(|err| super::Error::Protocol(format!("cannot encode text payload: {err}")))?;
    Ok(out.into_inner())
}

fn uid(index: u64) -> Value {
    Value::Uid(Uid::new(index))
}

fn dict(pairs: Vec<(&str, Value)>) -> Value {
    let mut out = Dictionary::new();
    for (key, value) in pairs {
        out.insert(key.to_string(), value);
    }
    Value::Dictionary(out)
}

fn class(name: &str) -> Value {
    dict(vec![
        ("$classname", Value::String(name.to_string())),
        (
            "$classes",
            Value::Array(vec![
                Value::String(name.to_string()),
                Value::String("NSObject".into()),
            ]),
        ),
    ])
}

fn keyboard_output_class() -> Value {
    class("TIKeyboardOutput")
}

fn uuid_class() -> Value {
    class("NSUUID")
}

fn text_operations_class() -> Value {
    class("RTITextOperations")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payloads_are_plists_with_the_expected_graph() {
        let uuid = [7u8; 16];
        let clear = clear_payload(&uuid).unwrap();
        let parsed = Value::from_reader(Cursor::new(&clear)).unwrap();
        assert_eq!(
            find(&parsed, "$archiver").and_then(|value| value.as_string()),
            Some("RTIKeyedArchiver")
        );
        assert_eq!(
            find(&parsed, "$archiver").and_then(|v| v.as_string()),
            Some("RTIKeyedArchiver")
        );
        let input = input_payload(&uuid, "hello").unwrap();
        let parsed = Value::from_reader(Cursor::new(&input)).unwrap();
        assert_eq!(
            find(&parsed, "insertionText").and_then(|v| v.as_string()),
            Some("hello")
        );
    }

    #[test]
    fn parses_a_session_archive() {
        let uuid = [3u8; 16];
        let payload = input_payload(&uuid, "hi").unwrap();
        let parsed = parse_text_input(&payload).unwrap();
        assert_eq!(parsed.session_uuid, uuid);
        assert!(parsed.current_text.is_empty() || parsed.current_text == "hi");
    }
}
