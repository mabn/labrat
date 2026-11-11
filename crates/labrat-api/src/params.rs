//! Request parameters merged from the query string and the body, with Grape-style coercion
//! and error messages.

use labrat_core::{Error, Result};
use serde_json::{Map, Value};

#[derive(Debug, Default, Clone)]
pub struct Params {
    pub map: Map<String, Value>,
}

/// Parses `a=1&b[]=2&c[d]=3` into nested JSON the way Rack does.
pub fn parse_query(q: &str, into: &mut Map<String, Value>) {
    for (k, v) in url::form_urlencoded::parse(q.as_bytes()) {
        insert_rack(into, &k, Value::String(v.into_owned()));
    }
}

fn insert_rack(map: &mut Map<String, Value>, key: &str, value: Value) {
    let Some(bracket) = key.find('[') else {
        map.insert(key.to_string(), value);
        return;
    };
    let (head, rest) = key.split_at(bracket);
    if rest == "[]" {
        let entry = map
            .entry(head.to_string())
            .or_insert_with(|| Value::Array(vec![]));
        if let Value::Array(a) = entry {
            a.push(value);
        } else {
            *entry = Value::Array(vec![value]);
        }
        return;
    }
    if let Some(end) = rest.find(']') {
        let inner = &rest[1..end];
        let tail = &rest[end + 1..];
        let entry = map
            .entry(head.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        let sub = entry.as_object_mut().unwrap();
        let next_key = format!("{inner}{tail}");
        insert_rack(sub, &next_key, value);
    } else {
        map.insert(key.to_string(), value);
    }
}

impl Params {
    pub fn has(&self, name: &str) -> bool {
        self.map.contains_key(name)
    }

    /// All parameters as one JSON value (a non-object JSON body comes back as itself).
    pub fn raw_all(&self) -> Value {
        match self.map.get("_json") {
            Some(v @ Value::Array(_)) => v.clone(),
            _ => Value::Object(self.map.clone()),
        }
    }

    pub fn raw(&self, name: &str) -> Option<&Value> {
        self.map.get(name)
    }

    /// Fails with Grape's "x is missing, y is missing" for absent required params.
    pub fn require(&self, names: &[&str]) -> Result<()> {
        let missing: Vec<String> = names
            .iter()
            .filter(|n| match self.map.get(**n) {
                None | Some(Value::Null) => true,
                _ => false,
            })
            .map(|n| format!("{n} is missing"))
            .collect();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(Error::Param(missing.join(", ")))
        }
    }

    /// Fails with "x, y are missing, at least one parameter must be provided".
    pub fn require_any(&self, names: &[&str]) -> Result<()> {
        if names.iter().any(|n| self.has(n)) {
            Ok(())
        } else {
            Err(Error::Param(format!(
                "{} are missing, at least one parameter must be provided",
                names.join(", ")
            )))
        }
    }

    pub fn str(&self, name: &str) -> Option<String> {
        match self.map.get(name)? {
            Value::String(s) => Some(s.clone()),
            Value::Null => None,
            Value::Bool(b) => Some(b.to_string()),
            Value::Number(n) => Some(n.to_string()),
            other => Some(other.to_string()),
        }
    }

    /// A string param where an explicit empty value or null means "clear".
    pub fn nullable_str(&self, name: &str) -> Option<Option<String>> {
        match self.map.get(name)? {
            Value::Null => Some(None),
            _ => Some(self.str(name)),
        }
    }

    pub fn bool(&self, name: &str) -> Result<Option<bool>> {
        let Some(v) = self.map.get(name) else {
            return Ok(None);
        };
        match v {
            Value::Bool(b) => Ok(Some(*b)),
            Value::Null => Ok(None),
            Value::Number(n) => match n.as_i64() {
                Some(1) => Ok(Some(true)),
                Some(0) => Ok(Some(false)),
                _ => Err(Error::Param(format!("{name} is invalid"))),
            },
            Value::String(s) => match s.to_ascii_lowercase().as_str() {
                "true" | "t" | "1" | "yes" | "y" | "on" => Ok(Some(true)),
                "false" | "f" | "0" | "no" | "n" | "off" | "" => Ok(Some(false)),
                _ => Err(Error::Param(format!("{name} is invalid"))),
            },
            _ => Err(Error::Param(format!("{name} is invalid"))),
        }
    }

    pub fn bool_or(&self, name: &str, default: bool) -> Result<bool> {
        Ok(self.bool(name)?.unwrap_or(default))
    }

    pub fn int(&self, name: &str) -> Result<Option<i64>> {
        let Some(v) = self.map.get(name) else {
            return Ok(None);
        };
        match v {
            Value::Number(n) => n
                .as_i64()
                .map(Some)
                .ok_or_else(|| Error::Param(format!("{name} is invalid"))),
            Value::Null => Ok(None),
            Value::String(s) => s
                .trim()
                .parse::<i64>()
                .map(Some)
                .map_err(|_| Error::Param(format!("{name} is invalid"))),
            _ => Err(Error::Param(format!("{name} is invalid"))),
        }
    }

    pub fn int_or(&self, name: &str, default: i64) -> Result<i64> {
        Ok(self.int(name)?.unwrap_or(default))
    }

    /// A string restricted to `allowed`, with Grape's "does not have a valid value" error.
    pub fn one_of(&self, name: &str, allowed: &[&str]) -> Result<Option<String>> {
        match self.str(name) {
            None => Ok(None),
            Some(v) if allowed.contains(&v.as_str()) => Ok(Some(v)),
            Some(_) => Err(Error::Param(format!("{name} does not have a valid value"))),
        }
    }

    /// Array params given as `x[]=a&x[]=b`, a JSON array, or a comma-separated string.
    pub fn list(&self, name: &str) -> Option<Vec<String>> {
        match self.map.get(name)? {
            Value::Array(a) => Some(
                a.iter()
                    .filter_map(|v| match v {
                        Value::String(s) => Some(s.clone()),
                        Value::Number(n) => Some(n.to_string()),
                        _ => None,
                    })
                    .collect(),
            ),
            Value::String(s) => Some(
                s.split(',')
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
                    .collect(),
            ),
            Value::Number(n) => Some(vec![n.to_string()]),
            Value::Null => Some(vec![]),
            _ => None,
        }
    }

    pub fn int_list(&self, name: &str) -> Result<Option<Vec<i64>>> {
        match self.list(name) {
            None => Ok(None),
            Some(v) => v
                .iter()
                .map(|s| {
                    s.parse::<i64>()
                        .map_err(|_| Error::Param(format!("{name} is invalid")))
                })
                .collect::<Result<Vec<_>>>()
                .map(Some),
        }
    }

    /// A date (`YYYY-MM-DD`) param.
    pub fn date(&self, name: &str) -> Result<Option<String>> {
        match self.str(name) {
            None => Ok(None),
            Some(s) if s.is_empty() => Ok(None),
            Some(s) => match labrat_core::time::parse_date(&s) {
                Some(d) => Ok(Some(d.format("%Y-%m-%d").to_string())),
                None => Err(Error::Param(format!("{name} is invalid"))),
            },
        }
    }

    /// A timestamp param, returned in milliseconds.
    pub fn time(&self, name: &str) -> Result<Option<i64>> {
        match self.str(name) {
            None => Ok(None),
            Some(s) => labrat_core::time::parse_time(&s)
                .map(Some)
                .ok_or_else(|| Error::Param(format!("{name} is invalid"))),
        }
    }
}

/// A file part of a `multipart/form-data` request.
#[derive(Debug, Clone)]
pub struct UploadedFile {
    pub field: String,
    pub filename: String,
    pub content_type: String,
    pub data: Vec<u8>,
}

/// Parses a `multipart/form-data` body: text parts become parameters, file parts are returned.
pub fn parse_multipart(ctype: &str, body: &[u8], into: &mut Map<String, Value>) -> Vec<UploadedFile> {
    let Some(boundary) = ctype.split(';').map(str::trim).find_map(|p| p.strip_prefix("boundary=")).map(|b| b.trim_matches('"').to_string()) else {
        return vec![];
    };
    let delim = format!("--{boundary}").into_bytes();
    let find = |hay: &[u8], needle: &[u8], from: usize| -> Option<usize> {
        if needle.is_empty() || hay.len() < needle.len() {
            return None;
        }
        (from..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
    };
    let mut files = vec![];
    let mut pos = match find(body, &delim, 0) {
        Some(p) => p + delim.len(),
        None => return files,
    };
    loop {
        if body.get(pos..pos + 2) == Some(b"--") {
            break;
        }
        // Skip the CRLF after the delimiter.
        if body.get(pos..pos + 2) == Some(b"\r\n") {
            pos += 2;
        }
        let Some(head_end) = find(body, b"\r\n\r\n", pos) else { break };
        let headers = String::from_utf8_lossy(&body[pos..head_end]).to_string();
        let data_start = head_end + 4;
        let Some(next) = find(body, &delim, data_start) else { break };
        let mut data_end = next;
        if data_end >= 2 && &body[data_end - 2..data_end] == b"\r\n" {
            data_end -= 2;
        }
        let data = body[data_start..data_end].to_vec();
        let mut name = String::new();
        let mut filename: Option<String> = None;
        let mut part_type = "application/octet-stream".to_string();
        for line in headers.split("\r\n") {
            let lower = line.to_ascii_lowercase();
            if lower.starts_with("content-disposition:") {
                for attr in line.split(';').map(str::trim) {
                    if let Some(v) = attr.strip_prefix("name=") {
                        name = v.trim_matches('"').to_string();
                    } else if let Some(v) = attr.strip_prefix("filename=") {
                        filename = Some(v.trim_matches('"').to_string());
                    }
                }
            } else if lower.starts_with("content-type:") {
                part_type = line[13..].trim().to_string();
            }
        }
        match filename {
            Some(f) => files.push(UploadedFile { field: name, filename: f, content_type: part_type, data }),
            None => {
                into.insert(name, Value::String(String::from_utf8_lossy(&data).into_owned()));
            }
        }
        pos = next + delim.len();
    }
    files
}
