//! [`WatchSource`] trait and built-in implementations.
//!
//! Each call to [`WatchSource::poll`] returns zero or more [`Match`]
//! values.  A `Match` is an opaque JSON object — the runner fingerprints
//! it for deduplication and forwards it verbatim in the `WatchEvent`.
//!
//! ## Built-in sources
//!
//! | Source       | How it works |
//! |--------------|--------------|
//! | [`SqlSource`]  | Runs a SELECT on `ReadWritePool::reader()`; each row → one `Match`. |
//! | [`HttpSource`] | GETs (or POSTs) an HTTP endpoint; applies a JSONPath selector to extract an array of objects. |

use async_trait::async_trait;
use serde_json::Value;
use sqlx::{Column, Row, ValueRef};
use thiserror::Error;
use tracing::instrument;

use crate::spec::WatchSourceSpec;

/// A single match returned by a [`WatchSource`].
///
/// The inner `Value` is **always** a JSON object (map).  Sources that
/// return non-object values wrap them in `{"value": <v>}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match(pub serde_json::Map<String, Value>);

impl Match {
    /// Construct from an arbitrary JSON value.  If the value is already
    /// an object it is used directly; otherwise it is wrapped.
    #[must_use]
    pub fn from_value(v: Value) -> Self {
        match v {
            Value::Object(m) => Self(m),
            other => {
                let mut m = serde_json::Map::new();
                m.insert("value".to_string(), other);
                Self(m)
            }
        }
    }

    /// View the match as a JSON value reference.
    #[must_use]
    pub fn as_value(&self) -> Value {
        Value::Object(self.0.clone())
    }
}

/// Error type returned by source implementations.
#[derive(Debug, Error)]
pub enum SourceError {
    #[error("sql error: {0}")]
    Sql(#[from] sqlx::Error),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("json parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("source config error: {0}")]
    Config(String),
    #[error("jsonpath error: {0}")]
    JsonPath(String),
}

/// Polling source trait.
///
/// Each implementation encapsulates one kind of data source.  The runner
/// calls `poll()` on every tick for each [`crate::spec::WatchSpec`].
#[async_trait]
pub trait WatchSource: Send + Sync {
    /// Poll the source and return all current matches.
    ///
    /// # Errors
    ///
    /// Returns [`SourceError`] when the underlying transport fails.
    async fn poll(&self) -> Result<Vec<Match>, SourceError>;
}

// ---------------------------------------------------------------------------
// SqlSource
// ---------------------------------------------------------------------------

/// Runs a parameterless SQL SELECT against the read pool and converts
/// each row to a JSON object.
///
/// Column names become object keys; values are serialised via `sqlx`'s
/// built-in JSON support.
pub struct SqlSource {
    pool: sqlx::SqlitePool,
    query: String,
}

impl SqlSource {
    /// Create from an existing pool and a validated SELECT query.
    ///
    /// Use `ReadWritePool::reader()` to obtain the pool reference:
    ///
    /// ```ignore
    /// let src = SqlSource::new(rw_pool.reader().clone(), spec.source);
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`SourceError::Config`] when `spec` is not a `Sql` variant.
    pub fn new(pool: sqlx::SqlitePool, spec: &WatchSourceSpec) -> Result<Self, SourceError> {
        match spec {
            WatchSourceSpec::Sql { query } => Ok(Self {
                pool,
                query: query.clone(),
            }),
            other @ WatchSourceSpec::Http { .. } => Err(SourceError::Config(format!(
                "SqlSource requires a Sql spec, got: {other:?}"
            ))),
        }
    }
}

#[async_trait]
impl WatchSource for SqlSource {
    #[instrument(skip(self), fields(query = %self.query))]
    async fn poll(&self) -> Result<Vec<Match>, SourceError> {
        // SQLite has no `row_to_json`, so we run the user SELECT directly and
        // decode each row generically: column names become object keys and
        // values are mapped to JSON based on the column's runtime type. This
        // is the most reliable approach for arbitrary user-supplied SELECTs.
        let rows = sqlx::query(&self.query).fetch_all(&self.pool).await?;

        let matches = rows
            .into_iter()
            .map(|row| Match(sqlite_row_to_json(&row)))
            .collect();
        Ok(matches)
    }
}

/// Convert a SQLite row into a JSON object map.
///
/// Each column becomes a key. Values are decoded by the column's runtime
/// type info; `NULL` becomes `Value::Null`. Unknown/text-storage values
/// fall back to a UTF-8 string.
fn sqlite_row_to_json(row: &sqlx::sqlite::SqliteRow) -> serde_json::Map<String, Value> {
    let mut map = serde_json::Map::new();
    for col in row.columns() {
        let name = col.name();
        let value = sqlite_value_to_json(row, col);
        map.insert(name.to_string(), value);
    }
    map
}

fn sqlite_value_to_json(row: &sqlx::sqlite::SqliteRow, col: &sqlx::sqlite::SqliteColumn) -> Value {
    let idx = col.ordinal();
    // NULL check via the raw value reference.
    if let Ok(raw) = row.try_get_raw(idx) {
        if raw.is_null() {
            return Value::Null;
        }
    }
    // Prefer a native JSON decode (handles json columns); then fall back
    // across the SQLite storage classes by attempting typed `try_get`s.
    if let Ok(v) = row.try_get::<Value, _>(idx) {
        return v;
    }
    if let Ok(v) = row.try_get::<i64, _>(idx) {
        return Value::from(v);
    }
    if let Ok(v) = row.try_get::<f64, _>(idx) {
        return Value::from(v);
    }
    if let Ok(v) = row.try_get::<bool, _>(idx) {
        return Value::from(v);
    }
    if let Ok(v) = row.try_get::<String, _>(idx) {
        return Value::from(v);
    }
    Value::Null
}

// ---------------------------------------------------------------------------
// HttpSource
// ---------------------------------------------------------------------------

/// Polls an HTTP endpoint and extracts matches via a minimal `JSONPath`
/// implementation (direct array element extraction).
///
/// The `JSONPath` support covers the common `$[*]` (all elements of a
/// top-level array) and `$.key[*]` (all elements of a named key) patterns
/// that appear in practice.  Full `JSONPath` is deferred to v1.3.x when a
/// dedicated crate is vetted.
pub struct HttpSource {
    client: reqwest::Client,
    url: String,
    method: String,
    jsonpath: String,
}

impl HttpSource {
    /// Construct from a spec and a shared `reqwest::Client`.
    ///
    /// # Errors
    ///
    /// Returns [`SourceError::Config`] when `spec` is not an `Http` variant.
    pub fn new(client: reqwest::Client, spec: &WatchSourceSpec) -> Result<Self, SourceError> {
        match spec {
            WatchSourceSpec::Http {
                url,
                jsonpath,
                method,
            } => Ok(Self {
                client,
                url: url.clone(),
                jsonpath: jsonpath.clone(),
                method: method.clone(),
            }),
            other @ WatchSourceSpec::Sql { .. } => Err(SourceError::Config(format!(
                "HttpSource requires an Http spec, got: {other:?}"
            ))),
        }
    }

    /// Apply the stored `JSONPath` expression to a JSON value.
    ///
    /// Supports:
    /// - `$[*]`    — iterate all elements of a top-level array
    /// - `$.KEY[*]` — iterate all elements of `response[KEY]`
    ///
    /// Any value that is not an array is treated as a single-element array.
    fn apply_jsonpath(&self, body: Value) -> Vec<Value> {
        let path = self.jsonpath.trim();
        if path == "$[*]" || path == "$.*" {
            return Self::to_vec(body);
        }
        // $.KEY[*] pattern
        if let Some(stripped) = path.strip_prefix("$.") {
            let key = stripped.trim_end_matches("[*]").trim_end_matches(".*");
            if !key.is_empty() {
                let extracted = body.get(key).cloned().unwrap_or(Value::Null);
                return Self::to_vec(extracted);
            }
        }
        // Fallback: treat the whole body as a single match
        vec![body]
    }

    fn to_vec(v: Value) -> Vec<Value> {
        match v {
            Value::Array(arr) => arr,
            Value::Null => vec![],
            other => vec![other],
        }
    }
}

#[async_trait]
impl WatchSource for HttpSource {
    #[instrument(skip(self), fields(url = %self.url, method = %self.method))]
    async fn poll(&self) -> Result<Vec<Match>, SourceError> {
        let request = match self.method.to_ascii_uppercase().as_str() {
            "POST" => self.client.post(&self.url),
            _ => self.client.get(&self.url),
        };
        let body: Value = request.send().await?.json().await?;
        let items = self.apply_jsonpath(body);
        Ok(items.into_iter().map(Match::from_value).collect())
    }
}

// ---------------------------------------------------------------------------
// InMemorySource (test helper)
// ---------------------------------------------------------------------------

/// Test-only source backed by a static list of matches.
///
/// Each call to `poll()` returns the same slice.  Useful for deterministic
/// unit and integration tests without a live database.
pub struct InMemorySource {
    rows: Vec<Match>,
}

impl InMemorySource {
    /// Construct with a fixed set of rows.
    #[must_use]
    pub fn new(rows: Vec<serde_json::Map<String, Value>>) -> Self {
        Self {
            rows: rows.into_iter().map(Match).collect(),
        }
    }
}

#[async_trait]
impl WatchSource for InMemorySource {
    async fn poll(&self) -> Result<Vec<Match>, SourceError> {
        Ok(self.rows.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn match_from_object_value_is_identity() {
        let obj = json!({"a": 1, "b": "x"});
        let m = Match::from_value(obj.clone());
        assert_eq!(m.as_value(), obj);
    }

    #[test]
    fn match_from_scalar_wraps_in_value_key() {
        let m = Match::from_value(json!(42));
        assert_eq!(m.as_value(), json!({"value": 42}));
    }

    #[tokio::test]
    async fn in_memory_source_returns_all_rows() {
        let rows = vec![
            serde_json::from_value(json!({"id": 1})).unwrap(),
            serde_json::from_value(json!({"id": 2})).unwrap(),
        ];
        let src = InMemorySource::new(rows);
        let matches = src.poll().await.unwrap();
        assert_eq!(matches.len(), 2);
    }

    #[tokio::test]
    async fn in_memory_source_empty() {
        let src = InMemorySource::new(vec![]);
        let matches = src.poll().await.unwrap();
        assert!(matches.is_empty());
    }

    #[test]
    fn http_source_jsonpath_top_level_array() {
        let client = reqwest::Client::new();
        let spec = WatchSourceSpec::Http {
            url: "http://example.com".into(),
            jsonpath: "$[*]".into(),
            method: "GET".into(),
        };
        let src = HttpSource::new(client, &spec).unwrap();
        let items = src.apply_jsonpath(json!([{"a": 1}, {"a": 2}]));
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn http_source_jsonpath_named_key() {
        let client = reqwest::Client::new();
        let spec = WatchSourceSpec::Http {
            url: "http://example.com".into(),
            jsonpath: "$.data[*]".into(),
            method: "GET".into(),
        };
        let src = HttpSource::new(client, &spec).unwrap();
        let items = src.apply_jsonpath(json!({"data": [{"id": 1}, {"id": 2}]}));
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn http_source_jsonpath_null_body_returns_empty() {
        let client = reqwest::Client::new();
        let spec = WatchSourceSpec::Http {
            url: "http://example.com".into(),
            jsonpath: "$[*]".into(),
            method: "GET".into(),
        };
        let src = HttpSource::new(client, &spec).unwrap();
        let items = src.apply_jsonpath(Value::Null);
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn sql_source_rejects_http_spec() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        let spec = WatchSourceSpec::Http {
            url: "http://x".into(),
            jsonpath: "$[*]".into(),
            method: "GET".into(),
        };
        assert!(SqlSource::new(pool, &spec).is_err());
    }

    #[test]
    fn http_source_rejects_sql_spec() {
        let client = reqwest::Client::new();
        let spec = WatchSourceSpec::Sql {
            query: "SELECT 1".into(),
        };
        assert!(HttpSource::new(client, &spec).is_err());
    }
}
