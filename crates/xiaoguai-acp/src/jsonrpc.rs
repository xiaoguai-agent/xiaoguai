//! Minimal JSON-RPC 2.0 envelope.
//!
//! Deliberately hand-rolled rather than reusing the schema crate's generic
//! `rpc` module: the adapter only needs to (a) parse an incoming line into
//! `{id?, method, params}` and (b) emit a success/error response or a
//! notification. JSON-RPC 2.0 is a fixed, trivial envelope — this is not
//! "guessing a wire protocol"; the *message contracts* still come from the
//! schema crate.

use serde::Deserialize;
use serde_json::Value;

/// Standard JSON-RPC 2.0 error codes (and the range we use).
pub mod codes {
    /// Invalid JSON was received.
    pub const PARSE_ERROR: i64 = -32700;
    /// The JSON sent is not a valid Request object.
    pub const INVALID_REQUEST: i64 = -32600;
    /// The method does not exist / is not supported.
    pub const METHOD_NOT_FOUND: i64 = -32601;
    /// Invalid method parameter(s).
    pub const INVALID_PARAMS: i64 = -32602;
    /// Internal JSON-RPC / handler error.
    pub const INTERNAL_ERROR: i64 = -32603;
}

/// An incoming message. A request carries an `id`; a notification omits it.
#[derive(Debug, Clone, Deserialize)]
pub struct Incoming {
    /// Present and `"2.0"` on conforming peers; tolerated when absent.
    #[serde(default)]
    pub jsonrpc: Option<String>,
    /// `Some` ⇒ request (expects a response); `None` ⇒ notification.
    #[serde(default)]
    pub id: Option<Value>,
    /// The method name (e.g. `session/prompt`).
    pub method: String,
    /// Method parameters; `Null` when omitted.
    #[serde(default)]
    pub params: Value,
}

impl Incoming {
    /// `true` when this message expects a response.
    #[must_use]
    pub fn is_request(&self) -> bool {
        self.id.is_some()
    }
}

/// A successful response to a request `id`.
#[must_use]
pub fn success(id: Value, result: Value) -> Value {
    let mut obj = serde_json::Map::with_capacity(3);
    obj.insert("jsonrpc".into(), Value::from("2.0"));
    obj.insert("id".into(), id);
    obj.insert("result".into(), result);
    Value::Object(obj)
}

/// An error response. `id` is `Null` when it could not be recovered.
#[must_use]
pub fn error(id: Value, code: i64, message: impl Into<String>) -> Value {
    let mut err = serde_json::Map::with_capacity(2);
    err.insert("code".into(), Value::from(code));
    err.insert("message".into(), Value::from(message.into()));
    let mut obj = serde_json::Map::with_capacity(3);
    obj.insert("jsonrpc".into(), Value::from("2.0"));
    obj.insert("id".into(), id);
    obj.insert("error".into(), Value::Object(err));
    Value::Object(obj)
}

/// A server→client notification (no `id`, no response expected).
#[must_use]
pub fn notification(method: &str, params: Value) -> Value {
    let mut obj = serde_json::Map::with_capacity(3);
    obj.insert("jsonrpc".into(), Value::from("2.0"));
    obj.insert("method".into(), Value::from(method));
    obj.insert("params".into(), params);
    Value::Object(obj)
}

/// Serialize a response/notification value, deferring the framing newline to
/// the transport.
///
/// # Errors
/// Returns the underlying `serde_json` error if the value is not serializable.
pub fn to_line(value: &Value) -> Result<String, serde_json::Error> {
    serde_json::to_string(value)
}
