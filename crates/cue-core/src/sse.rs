//! Minimal, spec-conformant Server-Sent Events parser plus a helper that drives
//! a streaming HTTP response with cancellation support.

use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::error::{CueError, ErrorKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
    event: Option<String>,
    data: Vec<String>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed raw bytes; returns all events completed by this chunk.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=pos).collect();
            line.pop(); // '\n'
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = String::from_utf8_lossy(&line).into_owned();
            if let Some(ev) = self.process_line(&line) {
                out.push(ev);
            }
        }
        out
    }

    /// Flush a trailing event that was not terminated by a blank line.
    pub fn finish(&mut self) -> Option<SseEvent> {
        if !self.buf.is_empty() {
            let line = String::from_utf8_lossy(&std::mem::take(&mut self.buf)).into_owned();
            let line = line.trim_end_matches('\r').to_string();
            if let Some(ev) = self.process_line(&line) {
                return Some(ev);
            }
        }
        self.dispatch()
    }

    fn process_line(&mut self, line: &str) -> Option<SseEvent> {
        if line.is_empty() {
            return self.dispatch();
        }
        if line.starts_with(':') {
            return None; // comment / keep-alive
        }
        let (field, value) = match line.find(':') {
            Some(i) => {
                let v = &line[i + 1..];
                (&line[..i], v.strip_prefix(' ').unwrap_or(v))
            }
            None => (line, ""),
        };
        match field {
            "event" => self.event = Some(value.to_string()),
            "data" => self.data.push(value.to_string()),
            _ => {} // id / retry are irrelevant here
        }
        None
    }

    fn dispatch(&mut self) -> Option<SseEvent> {
        if self.data.is_empty() {
            self.event = None;
            return None;
        }
        let ev = SseEvent { event: self.event.take(), data: self.data.join("\n") };
        self.data.clear();
        Some(ev)
    }
}

pub enum Flow {
    Continue,
    Stop,
}

/// Read an SSE response body to completion, calling `on_event` for each event.
/// Honors `cancel` and an idle timeout between chunks.
pub async fn drive_sse<F>(
    resp: reqwest::Response,
    provider: &str,
    cancel: &CancellationToken,
    mut on_event: F,
) -> Result<(), CueError>
where
    F: FnMut(SseEvent) -> Result<Flow, CueError>,
{
    const IDLE: std::time::Duration = std::time::Duration::from_secs(120);
    let mut stream = resp.bytes_stream();
    let mut parser = SseParser::new();
    loop {
        let next = tokio::select! {
            _ = cancel.cancelled() => return Err(CueError::cancelled()),
            r = tokio::time::timeout(IDLE, stream.next()) => r,
        };
        let chunk = match next {
            Err(_) => {
                return Err(CueError::from_kind(ErrorKind::Timeout, provider)
                    .with_detail("No data received from the stream for 120 seconds."));
            }
            Ok(None) => break,
            Ok(Some(Err(e))) => {
                return Err(CueError::from_reqwest(&e, provider)
                    .with_hint("The connection was interrupted while streaming. Try again."));
            }
            Ok(Some(Ok(b))) => b,
        };
        for ev in parser.push(&chunk) {
            if let Flow::Stop = on_event(ev)? {
                return Ok(());
            }
        }
    }
    if let Some(ev) = parser.finish() {
        on_event(ev)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_split_chunks_and_crlf() {
        let mut p = SseParser::new();
        let mut evs = p.push(b"event: a\r\ndata: {\"x\"");
        assert!(evs.is_empty());
        evs.extend(p.push(b":1}\r\n\r\n: keepalive\n\ndata: line1\ndata: line2\n\n"));
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].event.as_deref(), Some("a"));
        assert_eq!(evs[0].data, "{\"x\":1}");
        assert_eq!(evs[1].event, None);
        assert_eq!(evs[1].data, "line1\nline2");
    }

    #[test]
    fn finish_flushes_unterminated() {
        let mut p = SseParser::new();
        assert!(p.push(b"data: [DONE]").is_empty());
        assert_eq!(p.finish().unwrap().data, "[DONE]");
    }
}
