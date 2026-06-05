//! Newline-delimited JSON-RPC framing over any async byte streams.
//!
//! ACP over stdio is one JSON object per line, `\n`-terminated, with no
//! `Content-Length` header (confirmed against the upstream `Stdio` transport).
//! Both halves are generic over `AsyncRead`/`AsyncWrite`, so the protocol is
//! driven over an in-memory `tokio::io::duplex` pipe in tests — no subprocess.

use std::sync::Arc;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, Lines};
use tokio::sync::Mutex;

use crate::jsonrpc;

/// Reads one JSON-RPC message per line.
pub struct LineReader<R> {
    lines: Lines<BufReader<R>>,
}

impl<R: AsyncRead + Unpin> LineReader<R> {
    /// Wrap a reader.
    pub fn new(reader: R) -> Self {
        Self {
            lines: BufReader::new(reader).lines(),
        }
    }

    /// Read the next non-empty line, or `None` at EOF.
    ///
    /// # Errors
    /// Returns an I/O error if the underlying stream fails.
    pub async fn next_message(&mut self) -> std::io::Result<Option<String>> {
        while let Some(line) = self.lines.next_line().await? {
            if !line.trim().is_empty() {
                return Ok(Some(line));
            }
        }
        Ok(None)
    }
}

/// Writes JSON-RPC messages, one per line. Cloneable + internally synchronized
/// so a prompt turn's `session/update` notifications and the eventual response
/// can be emitted from concurrent tasks without interleaving bytes.
pub struct LineWriter<W> {
    inner: Arc<Mutex<W>>,
}

impl<W> Clone for LineWriter<W> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<W: AsyncWrite + Unpin> LineWriter<W> {
    /// Wrap a writer.
    pub fn new(writer: W) -> Self {
        Self {
            inner: Arc::new(Mutex::new(writer)),
        }
    }

    /// Serialize `value` and write it as one framed line, then flush.
    ///
    /// # Errors
    /// Returns an I/O error if serialization or the write fails.
    pub async fn write_message(&self, value: &Value) -> std::io::Result<()> {
        let mut line = jsonrpc::to_line(value)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        line.push('\n');
        let mut w = self.inner.lock().await;
        w.write_all(line.as_bytes()).await?;
        w.flush().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn round_trips_two_messages_over_duplex() {
        let (a, b) = tokio::io::duplex(1024);
        let writer = LineWriter::new(a);
        let mut reader = LineReader::new(b);

        writer
            .write_message(&serde_json::json!({"jsonrpc":"2.0","method":"ping"}))
            .await
            .unwrap();
        writer
            .write_message(&serde_json::json!({"jsonrpc":"2.0","method":"pong"}))
            .await
            .unwrap();

        let first: serde_json::Value =
            serde_json::from_str(&reader.next_message().await.unwrap().unwrap()).unwrap();
        let second: serde_json::Value =
            serde_json::from_str(&reader.next_message().await.unwrap().unwrap()).unwrap();
        assert_eq!(first["method"], "ping");
        assert_eq!(second["method"], "pong");
    }

    #[tokio::test]
    async fn skips_blank_lines_and_reports_eof() {
        let (a, b) = tokio::io::duplex(1024);
        let writer = LineWriter::new(a);
        writer
            .write_message(&serde_json::json!({"method":"only"}))
            .await
            .unwrap();
        drop(writer); // close the write half → EOF after the one message
        let mut reader = LineReader::new(b);
        assert_eq!(
            reader.next_message().await.unwrap().as_deref(),
            Some("{\"method\":\"only\"}")
        );
        assert!(reader.next_message().await.unwrap().is_none());
    }
}
