use std::io::{self, BufRead, Write};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Upper bound on a single message, so a misbehaving peer can't make us buffer forever.
pub const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

const VERSION: &str = "2.0";

fn version() -> String {
    VERSION.to_owned()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Request {
    #[serde(default = "version")]
    pub jsonrpc: String,
    pub id: Value,
    pub method: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub params: Value,
}

/// A request without an id: no response is sent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Notification {
    #[serde(default = "version")]
    pub jsonrpc: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Response {
    #[serde(default = "version")]
    pub jsonrpc: String,
    pub id: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;

    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), data: None }
    }

    pub fn method_not_found(method: &str) -> Self {
        Self::new(Self::METHOD_NOT_FOUND, format!("unknown method `{method}`"))
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(Self::INVALID_PARAMS, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(Self::INTERNAL_ERROR, message)
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (code {})", self.message, self.code)
    }
}

impl std::error::Error for RpcError {}

impl Response {
    pub fn ok(id: Value, result: Value) -> Self {
        Self { jsonrpc: version(), id, result: Some(result), error: None }
    }

    pub fn err(id: Value, error: RpcError) -> Self {
        Self { jsonrpc: version(), id, result: None, error: Some(error) }
    }

    pub fn into_result(self) -> Result<Value, RpcError> {
        match self.error {
            Some(e) => Err(e),
            None => Ok(self.result.unwrap_or(Value::Null)),
        }
    }
}

impl Request {
    pub fn new(id: impl Into<Value>, method: impl Into<String>, params: Value) -> Self {
        Self { jsonrpc: version(), id: id.into(), method: method.into(), params }
    }
}

impl Notification {
    pub fn new(method: impl Into<String>, params: Value) -> Self {
        Self { jsonrpc: version(), method: method.into(), params }
    }
}

/// Any JSON-RPC message, told apart by which fields are present.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Request(Request),
    Notification(Notification),
    Response(Response),
}

impl Message {
    pub fn parse(line: &str) -> Result<Self, RpcError> {
        let value: Value = serde_json::from_str(line)
            .map_err(|e| RpcError::new(RpcError::PARSE_ERROR, e.to_string()))?;
        let obj = value
            .as_object()
            .ok_or_else(|| RpcError::new(RpcError::INVALID_REQUEST, "message must be an object"))?;
        let invalid = |e: serde_json::Error| RpcError::new(RpcError::INVALID_REQUEST, e.to_string());
        if obj.contains_key("method") {
            if obj.get("id").is_some_and(|id| !id.is_null()) {
                serde_json::from_value(value).map(Message::Request).map_err(invalid)
            } else {
                serde_json::from_value(value).map(Message::Notification).map_err(invalid)
            }
        } else {
            serde_json::from_value(value).map(Message::Response).map_err(invalid)
        }
    }

    pub fn to_line(&self) -> String {
        let json = match self {
            Message::Request(r) => serde_json::to_string(r),
            Message::Notification(n) => serde_json::to_string(n),
            Message::Response(r) => serde_json::to_string(r),
        };
        // Serialising these plain structs cannot fail.
        json.unwrap_or_default()
    }
}

impl From<Request> for Message {
    fn from(r: Request) -> Self {
        Message::Request(r)
    }
}
impl From<Notification> for Message {
    fn from(n: Notification) -> Self {
        Message::Notification(n)
    }
}
impl From<Response> for Message {
    fn from(r: Response) -> Self {
        Message::Response(r)
    }
}

/// Read one newline-terminated line, capped at [`MAX_MESSAGE_BYTES`].
/// Returns `Ok(None)` on a clean end of stream.
pub fn read_line(reader: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut buf = Vec::new();
    let n = io::Read::take(&mut *reader, MAX_MESSAGE_BYTES as u64 + 1).read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(None);
    }
    if buf.len() > MAX_MESSAGE_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "message too large"));
    }
    while matches!(buf.last(), Some(b'\n' | b'\r')) {
        buf.pop();
    }
    String::from_utf8(buf).map(Some).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Read the next non-empty message. Lines that fail to parse are returned as `Err` in the
/// inner result so the caller can answer with a parse error and keep the connection.
pub fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Result<Message, RpcError>>> {
    loop {
        match read_line(reader)? {
            None => return Ok(None),
            Some(line) if line.trim().is_empty() => continue,
            Some(line) => return Ok(Some(Message::parse(&line))),
        }
    }
}

pub fn write_message(writer: &mut impl Write, msg: &Message) -> io::Result<()> {
    let mut line = msg.to_line();
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifies_messages() {
        let req = Message::parse(r#"{"jsonrpc":"2.0","id":1,"method":"reload"}"#).unwrap();
        assert!(matches!(req, Message::Request(r) if r.method == "reload" && r.params.is_null()));

        let note = Message::parse(r#"{"jsonrpc":"2.0","method":"publish","params":{"a":1}}"#).unwrap();
        assert!(matches!(note, Message::Notification(n) if n.params == json!({"a":1})));

        let resp = Message::parse(r#"{"jsonrpc":"2.0","id":"x","result":true}"#).unwrap();
        assert!(matches!(resp, Message::Response(r) if r.result == Some(json!(true))));
    }

    #[test]
    fn reports_parse_errors() {
        assert_eq!(Message::parse("{nope").unwrap_err().code, RpcError::PARSE_ERROR);
        assert_eq!(Message::parse("[1,2]").unwrap_err().code, RpcError::INVALID_REQUEST);
    }

    #[test]
    fn round_trips_over_a_stream() {
        let mut buf = Vec::new();
        let req = Message::from(Request::new(7, "status", json!({"verbose": true})));
        write_message(&mut buf, &req).unwrap();
        write_message(&mut buf, &Message::from(Response::err(json!(7), RpcError::method_not_found("x")))).unwrap();

        let mut reader = io::Cursor::new(buf);
        assert_eq!(read_message(&mut reader).unwrap().unwrap().unwrap(), req);
        let Message::Response(resp) = read_message(&mut reader).unwrap().unwrap().unwrap() else {
            panic!("expected response");
        };
        assert_eq!(resp.into_result().unwrap_err().code, RpcError::METHOD_NOT_FOUND);
        assert!(read_message(&mut reader).unwrap().is_none());
    }

    #[test]
    fn rejects_oversized_lines() {
        let big = vec![b'a'; MAX_MESSAGE_BYTES + 10];
        let mut reader = io::Cursor::new(big);
        assert_eq!(read_line(&mut reader).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }
}
