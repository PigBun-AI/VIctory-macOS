//! A small JSON reader for the lines of an input script (objects, arrays,
//! strings, numbers, booleans, null): enough for `VICTORY_INPUT_REPLAY`, with
//! no dependency (ported from VIctory-macOS `gameinput_script.rs`).

use std::collections::HashMap;

use super::super::KeyState;
use super::keys::key_named;

#[derive(Clone, Debug)]
pub(super) enum JsonVal {
    Bool(bool),
    Num(String),
    Array(Vec<JsonVal>),
    Object(HashMap<String, JsonVal>),
    Null,
    Str(String),
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> JsonParser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            bytes: input.as_bytes(),
            pos: 0,
        }
    }

    fn skip_ws(&mut self) {
        while let Some(&b) = self.bytes.get(self.pos) {
            if b.is_ascii_whitespace() {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn expect(&mut self, ch: u8) -> Result<(), String> {
        self.skip_ws();
        if self.bytes.get(self.pos) == Some(&ch) {
            self.pos += 1;
            Ok(())
        } else {
            Err(format!("expected '{}' at byte {}", ch as char, self.pos))
        }
    }

    fn parse_string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out = Vec::new();
        while let Some(&b) = self.bytes.get(self.pos) {
            self.pos += 1;
            match b {
                b'"' => {
                    return String::from_utf8(out)
                        .map_err(|e| format!("invalid UTF-8 in JSON string: {e}"));
                }
                b'\\' => {
                    let esc = *self
                        .bytes
                        .get(self.pos)
                        .ok_or_else(|| "unterminated escape".to_owned())?;
                    self.pos += 1;
                    match esc {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'/' => out.push(b'/'),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        _ => return Err(format!("unsupported escape \\{}", esc as char)),
                    }
                }
                c => out.push(c),
            }
        }
        Err("unterminated string".to_owned())
    }

    fn parse_number(&mut self) -> Result<String, String> {
        self.skip_ws();
        let start = self.pos;
        while let Some(&b) = self.bytes.get(self.pos) {
            if b.is_ascii_digit() || matches!(b, b'-' | b'+' | b'.' | b'e' | b'E') {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.pos == start {
            return Err(format!("expected number at byte {}", self.pos));
        }
        std::str::from_utf8(&self.bytes[start..self.pos])
            .map(str::to_owned)
            .map_err(|e| e.to_string())
    }

    fn parse_value(&mut self) -> Result<JsonVal, String> {
        self.skip_ws();
        let Some(&b) = self.bytes.get(self.pos) else {
            return Err("unexpected end of JSON".to_owned());
        };
        match b {
            b'{' => Ok(JsonVal::Object(self.parse_object()?)),
            b'[' => {
                self.pos += 1;
                let mut items = Vec::new();
                self.skip_ws();
                if self.bytes.get(self.pos) == Some(&b']') {
                    self.pos += 1;
                    return Ok(JsonVal::Array(items));
                }
                loop {
                    items.push(self.parse_value()?);
                    self.skip_ws();
                    match self.bytes.get(self.pos) {
                        Some(&b',') => self.pos += 1,
                        Some(&b']') => {
                            self.pos += 1;
                            return Ok(JsonVal::Array(items));
                        }
                        _ => return Err(format!("expected ',' or ']' at byte {}", self.pos)),
                    }
                }
            }
            b'"' => Ok(JsonVal::Str(self.parse_string()?)),
            b't' if self.bytes[self.pos..].starts_with(b"true") => {
                self.pos += 4;
                Ok(JsonVal::Bool(true))
            }
            b'f' if self.bytes[self.pos..].starts_with(b"false") => {
                self.pos += 5;
                Ok(JsonVal::Bool(false))
            }
            b'n' if self.bytes[self.pos..].starts_with(b"null") => {
                self.pos += 4;
                Ok(JsonVal::Null)
            }
            _ => Ok(JsonVal::Num(self.parse_number()?)),
        }
    }

    fn parse_object(&mut self) -> Result<HashMap<String, JsonVal>, String> {
        self.expect(b'{')?;
        let mut map = HashMap::new();
        self.skip_ws();
        if self.bytes.get(self.pos) == Some(&b'}') {
            self.pos += 1;
            return Ok(map);
        }
        loop {
            let key = self.parse_string()?;
            self.expect(b':')?;
            let val = self.parse_value()?;
            map.insert(key, val);
            self.skip_ws();
            match self.bytes.get(self.pos) {
                Some(&b',') => self.pos += 1,
                Some(&b'}') => {
                    self.pos += 1;
                    return Ok(map);
                }
                _ => return Err(format!("expected ',' or '}}' at byte {}", self.pos)),
            }
        }
    }
}

pub(super) fn parse_json_object(line: &str) -> Result<HashMap<String, JsonVal>, String> {
    let mut parser = JsonParser::new(line);
    let obj = parser.parse_object()?;
    parser.skip_ws();
    if parser.pos != parser.bytes.len() {
        return Err(format!("trailing characters at byte {}", parser.pos));
    }
    Ok(obj)
}

pub(super) fn obj_u64(obj: &HashMap<String, JsonVal>, key: &str) -> Result<Option<u64>, String> {
    match obj.get(key) {
        None | Some(JsonVal::Null) => Ok(None),
        Some(JsonVal::Num(s)) => s
            .parse::<u64>()
            .map(Some)
            .map_err(|e| format!("invalid integer `{key}` ({s}): {e}")),
        _ => Err(format!("expected integer for `{key}`")),
    }
}

pub(super) fn obj_f32(obj: &HashMap<String, JsonVal>, key: &str) -> Result<Option<f32>, String> {
    match obj.get(key) {
        None | Some(JsonVal::Null) => Ok(None),
        Some(JsonVal::Num(s)) => s
            .parse::<f32>()
            .map(Some)
            .map_err(|e| format!("invalid float `{key}` ({s}): {e}")),
        _ => Err(format!("expected float for `{key}`")),
    }
}

pub(super) fn obj_bool(obj: &HashMap<String, JsonVal>, key: &str) -> Result<Option<bool>, String> {
    match obj.get(key) {
        None | Some(JsonVal::Null) => Ok(None),
        Some(JsonVal::Bool(b)) => Ok(Some(*b)),
        _ => Err(format!("expected bool for `{key}`")),
    }
}

pub(super) fn obj_keys(obj: &HashMap<String, JsonVal>, key: &str) -> Result<Vec<KeyState>, String> {
    match obj.get(key) {
        None | Some(JsonVal::Null) => Ok(Vec::new()),
        Some(v) => parse_keys_value(v),
    }
}

pub(super) fn parse_keys_value(val: &JsonVal) -> Result<Vec<KeyState>, String> {
    let JsonVal::Array(arr) = val else {
        return Err("expected array for `keys`".to_owned());
    };
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        match item {
            JsonVal::Object(map) => {
                let vk = obj_u64(map, "vk")?
                    .or(obj_u64(map, "virtual_key")?)
                    .ok_or_else(|| "key object missing `vk`".to_owned())?
                    as u8;
                let scan = obj_u64(map, "scan")?
                    .or(obj_u64(map, "scan_code")?)
                    .unwrap_or(0) as u32;
                let cp = obj_u64(map, "cp")?
                    .or(obj_u64(map, "code_point")?)
                    .unwrap_or(0) as u32;
                let dead = obj_bool(map, "dead")?
                    .or(obj_bool(map, "dead_key")?)
                    .unwrap_or(false);
                out.push(KeyState {
                    scan_code: scan,
                    code_point: cp,
                    virtual_key: vk,
                    dead_key: dead,
                });
            }
            JsonVal::Num(s) => {
                let vk = s
                    .parse::<u8>()
                    .map_err(|e| format!("invalid key code {s}: {e}"))?;
                out.push(KeyState {
                    virtual_key: vk,
                    ..Default::default()
                });
            }
            // A key name of `VICTORY_KEYS` (`"Enter"`, `"Left"`, `"w"`).
            JsonVal::Str(s) if key_named(s).is_some() => {
                out.push(KeyState {
                    virtual_key: key_named(s).unwrap_or_default(),
                    ..Default::default()
                });
            }
            JsonVal::Str(s) if s.len() == 1 => {
                out.push(KeyState {
                    virtual_key: s.as_bytes()[0].to_ascii_uppercase(),
                    ..Default::default()
                });
            }
            _ => return Err("unsupported entry inside `keys`".to_owned()),
        }
    }
    Ok(out)
}
