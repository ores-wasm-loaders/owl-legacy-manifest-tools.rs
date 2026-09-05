//! A small JSON reader and writer.
//!
//! Only what a manifest needs: objects, arrays, strings, integers, booleans and null. Any
//! construct outside that — a float, an escape we do not implement — is an error rather
//! than a silent approximation, because this tool's whole job is to be exact about what a
//! release contains.

use std::collections::BTreeMap;
use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    Arr(Vec<Json>),
    Obj(BTreeMap<String, Json>),
}

#[derive(Debug, PartialEq, Eq)]
pub struct JsonError {
    pub message: String,
    pub offset: usize,
}

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at byte {}", self.message, self.offset)
    }
}

impl std::error::Error for JsonError {}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(map) => map.get(key),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Json::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(items) => Some(items),
            _ => None,
        }
    }

    /// Pretty-print with two-space indentation and sorted keys, so a regenerated manifest
    /// is byte-identical when nothing changed and a diff shows only what did.
    pub fn to_pretty(&self) -> String {
        let mut out = String::new();
        write_value(&mut out, self, 0);
        out.push('\n');
        out
    }
}

fn write_string(out: &mut String, value: &str) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn write_value(out: &mut String, value: &Json, depth: usize) {
    let pad = "  ".repeat(depth);
    let inner = "  ".repeat(depth + 1);
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Json::Int(i) => {
            let _ = write!(out, "{i}");
        }
        Json::Str(s) => write_string(out, s),
        Json::Arr(items) if items.is_empty() => out.push_str("[]"),
        Json::Arr(items) => {
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                out.push_str(&inner);
                write_value(out, item, depth + 1);
                if i + 1 < items.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&pad);
            out.push(']');
        }
        Json::Obj(map) if map.is_empty() => out.push_str("{}"),
        Json::Obj(map) => {
            out.push_str("{\n");
            for (i, (key, item)) in map.iter().enumerate() {
                out.push_str(&inner);
                write_string(out, key);
                out.push_str(": ");
                write_value(out, item, depth + 1);
                if i + 1 < map.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&pad);
            out.push('}');
        }
    }
}

pub fn parse(input: &str) -> Result<Json, JsonError> {
    let bytes = input.as_bytes();
    let mut pos = 0usize;
    let value = parse_value(bytes, &mut pos)?;
    skip_ws(bytes, &mut pos);
    if pos != bytes.len() {
        return Err(JsonError {
            message: "trailing input".into(),
            offset: pos,
        });
    }
    Ok(value)
}

fn skip_ws(bytes: &[u8], pos: &mut usize) {
    while *pos < bytes.len() && matches!(bytes[*pos], b' ' | b'\t' | b'\n' | b'\r') {
        *pos += 1;
    }
}

fn parse_value(bytes: &[u8], pos: &mut usize) -> Result<Json, JsonError> {
    skip_ws(bytes, pos);
    match bytes.get(*pos) {
        None => Err(JsonError {
            message: "unexpected end of input".into(),
            offset: *pos,
        }),
        Some(b'{') => parse_object(bytes, pos),
        Some(b'[') => parse_array(bytes, pos),
        Some(b'"') => parse_string(bytes, pos).map(Json::Str),
        Some(b't') => expect(bytes, pos, "true").map(|_| Json::Bool(true)),
        Some(b'f') => expect(bytes, pos, "false").map(|_| Json::Bool(false)),
        Some(b'n') => expect(bytes, pos, "null").map(|_| Json::Null),
        Some(_) => parse_int(bytes, pos),
    }
}

fn expect(bytes: &[u8], pos: &mut usize, word: &str) -> Result<(), JsonError> {
    if bytes[*pos..].starts_with(word.as_bytes()) {
        *pos += word.len();
        Ok(())
    } else {
        Err(JsonError {
            message: format!("expected `{word}`"),
            offset: *pos,
        })
    }
}

fn parse_int(bytes: &[u8], pos: &mut usize) -> Result<Json, JsonError> {
    let start = *pos;
    if bytes.get(*pos) == Some(&b'-') {
        *pos += 1;
    }
    while matches!(bytes.get(*pos), Some(b'0'..=b'9')) {
        *pos += 1;
    }
    if matches!(bytes.get(*pos), Some(b'.') | Some(b'e') | Some(b'E')) {
        return Err(JsonError {
            message: "fractional and exponent numbers are not part of the manifest contract".into(),
            offset: *pos,
        });
    }
    let text = std::str::from_utf8(&bytes[start..*pos]).map_err(|_| JsonError {
        message: "invalid utf-8".into(),
        offset: start,
    })?;
    text.parse::<i64>().map(Json::Int).map_err(|_| JsonError {
        message: format!("invalid number `{text}`"),
        offset: start,
    })
}

fn parse_string(bytes: &[u8], pos: &mut usize) -> Result<String, JsonError> {
    *pos += 1; // opening quote
    let mut out = String::new();
    loop {
        match bytes.get(*pos) {
            None => {
                return Err(JsonError {
                    message: "unterminated string".into(),
                    offset: *pos,
                })
            }
            Some(b'"') => {
                *pos += 1;
                return Ok(out);
            }
            Some(b'\\') => {
                *pos += 1;
                let escape = bytes.get(*pos).copied().ok_or(JsonError {
                    message: "dangling escape".into(),
                    offset: *pos,
                })?;
                *pos += 1;
                match escape {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'b' => out.push('\u{8}'),
                    b'f' => out.push('\u{c}'),
                    b'u' => {
                        let hex =
                            std::str::from_utf8(bytes.get(*pos..*pos + 4).ok_or(JsonError {
                                message: "short \\u escape".into(),
                                offset: *pos,
                            })?)
                            .map_err(|_| JsonError {
                                message: "invalid utf-8".into(),
                                offset: *pos,
                            })?;
                        let code = u32::from_str_radix(hex, 16).map_err(|_| JsonError {
                            message: "invalid \\u escape".into(),
                            offset: *pos,
                        })?;
                        out.push(char::from_u32(code).ok_or(JsonError {
                            message: "invalid code point".into(),
                            offset: *pos,
                        })?);
                        *pos += 4;
                    }
                    other => {
                        return Err(JsonError {
                            message: format!("unsupported escape `\\{}`", other as char),
                            offset: *pos,
                        })
                    }
                }
            }
            Some(_) => {
                let rest = std::str::from_utf8(&bytes[*pos..]).map_err(|_| JsonError {
                    message: "invalid utf-8".into(),
                    offset: *pos,
                })?;
                let ch = rest.chars().next().expect("non-empty");
                out.push(ch);
                *pos += ch.len_utf8();
            }
        }
    }
}

fn parse_array(bytes: &[u8], pos: &mut usize) -> Result<Json, JsonError> {
    *pos += 1;
    let mut items = Vec::new();
    skip_ws(bytes, pos);
    if bytes.get(*pos) == Some(&b']') {
        *pos += 1;
        return Ok(Json::Arr(items));
    }
    loop {
        items.push(parse_value(bytes, pos)?);
        skip_ws(bytes, pos);
        match bytes.get(*pos) {
            Some(b',') => *pos += 1,
            Some(b']') => {
                *pos += 1;
                return Ok(Json::Arr(items));
            }
            _ => {
                return Err(JsonError {
                    message: "expected `,` or `]`".into(),
                    offset: *pos,
                })
            }
        }
    }
}

fn parse_object(bytes: &[u8], pos: &mut usize) -> Result<Json, JsonError> {
    *pos += 1;
    let mut map = BTreeMap::new();
    skip_ws(bytes, pos);
    if bytes.get(*pos) == Some(&b'}') {
        *pos += 1;
        return Ok(Json::Obj(map));
    }
    loop {
        skip_ws(bytes, pos);
        if bytes.get(*pos) != Some(&b'"') {
            return Err(JsonError {
                message: "expected a property name".into(),
                offset: *pos,
            });
        }
        let key = parse_string(bytes, pos)?;
        skip_ws(bytes, pos);
        if bytes.get(*pos) != Some(&b':') {
            return Err(JsonError {
                message: "expected `:`".into(),
                offset: *pos,
            });
        }
        *pos += 1;
        let value = parse_value(bytes, pos)?;
        if map.insert(key.clone(), value).is_some() {
            return Err(JsonError {
                message: format!("duplicate property `{key}`"),
                offset: *pos,
            });
        }
        skip_ws(bytes, pos);
        match bytes.get(*pos) {
            Some(b',') => *pos += 1,
            Some(b'}') => {
                *pos += 1;
                return Ok(Json::Obj(map));
            }
            _ => {
                return Err(JsonError {
                    message: "expected `,` or `}`".into(),
                    offset: *pos,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{parse, Json};

    #[test]
    fn round_trips_a_manifest_shaped_document() {
        let text = r#"{"a":[1,2,{"b":"x"}],"c":true,"d":null,"e":-7}"#;
        let value = parse(text).expect("parses");
        let pretty = value.to_pretty();
        assert_eq!(parse(&pretty).expect("re-parses"), value);
        assert_eq!(value.get("e").and_then(Json::as_int), Some(-7));
        assert_eq!(value.get("c").and_then(Json::as_bool), Some(true));
    }

    #[test]
    fn refuses_what_it_cannot_represent_exactly() {
        assert!(
            parse("{\"a\": 1.5}").is_err(),
            "floats are not part of the contract"
        );
        assert!(parse("{\"a\": 1, \"a\": 2}").is_err(), "duplicate keys");
        assert!(parse("{\"a\": 1} trailing").is_err());
        assert!(parse("{\"a\": \"\\x\"}").is_err(), "unsupported escape");
    }

    #[test]
    fn keys_are_written_in_a_stable_order() {
        let value = parse(r#"{"z":1,"a":2}"#).expect("parses");
        assert!(
            value.to_pretty().find("\"a\"").unwrap() < value.to_pretty().find("\"z\"").unwrap()
        );
    }
}
