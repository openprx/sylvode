//! The client side of the API's change-hint stream (`GET /api/v1/bot/change-hints`).
//!
//! The stream is Server-Sent Events. This module opens it with the caller's credential, waits for
//! `hello` and then hands out the events one at a time. It carries ids and event names only; the
//! listen hub decides what a hint means. Nothing here logs a token, an id or a payload.

use super::OpenPrClient;
use reqwest::{Client, StatusCode, header};
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::OnceLock;
use std::time::Duration;

/// Path of the stream on the API.
pub const HINTS_PATH: &str = "/api/v1/bot/change-hints";

/// How long opening the stream (request, status, `hello`) may take.
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest SSE line or event payload accepted; a longer one ends the stream.
const MAX_EVENT_BYTES: usize = 16 * 1024;

/// Why the stream could not be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintOpenError {
    /// The API does not accept the credential (`401`).
    Unauthorized,
    /// The credential is not a bot token that may read (`403`): there is no push for it.
    NotABot,
    /// The API holds too many streams for this bot or process (`429`).
    TooMany,
    /// Anything else: the API is down, shutting down, slow or answered something unexpected.
    Unavailable,
}

/// One event of the stream, reduced to what the listen hub acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintEvent {
    /// A task of this bot changed.
    Task,
    /// The bot's inbox changed.
    Inbox,
    /// A work item or its comments changed.
    Issue,
    /// An artefact draft changed.
    Draft,
    /// The tool surface may differ now (agent policy, project type).
    Tools,
    /// The bot's project access changed.
    AccessChanged,
    /// The credential is revoked, deactivated or expired; the stream ends.
    Revoked,
    /// The API dropped hints for this stream (back-pressure, lost listener); anything may have
    /// changed. The stream ends.
    Lagged,
    /// The API is shutting down; the stream ends.
    Shutdown,
    /// An event this client does not know; ignored.
    Unknown,
}

/// Why reading the stream failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintReadError {
    /// The connection broke.
    Transport,
    /// A line or an event was longer than [`MAX_EVENT_BYTES`].
    Oversized,
}

/// An open change-hint stream, past `hello`.
pub struct HintStream {
    response: reqwest::Response,
    parser: SseParser,
    ready: VecDeque<Frame>,
}

impl HintStream {
    /// The next event, `Ok(None)` when the API ended the stream cleanly. Cancel safe: dropping
    /// the future loses no event.
    pub async fn next_event(&mut self) -> Result<Option<HintEvent>, HintReadError> {
        loop {
            if let Some(frame) = self.ready.pop_front() {
                return Ok(Some(frame.event()));
            }
            match self.response.chunk().await {
                Ok(Some(chunk)) => self.parser.push(&chunk, &mut self.ready)?,
                Ok(None) => return Ok(None),
                Err(_) => return Err(HintReadError::Transport),
            }
        }
    }
}

/// The client used for streams: no total timeout (a stream lives as long as it lives), a bounded
/// connect time and no redirects, so the credential is never sent anywhere else.
fn stream_client() -> Option<&'static Client> {
    static CLIENT: OnceLock<Option<Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .ok()
        })
        .as_ref()
}

fn classify_code(code: i64) -> HintOpenError {
    match code {
        401 => HintOpenError::Unauthorized,
        403 => HintOpenError::NotABot,
        429 => HintOpenError::TooMany,
        _ => HintOpenError::Unavailable,
    }
}

impl OpenPrClient {
    /// Opens the change-hint stream of this client's credential and waits for `hello`.
    ///
    /// The API answers `200` with the usual envelope for errors that occur before the stream
    /// starts, and a real status for limits and shutdown; both are mapped the same way.
    pub async fn open_change_hints(&self) -> Result<HintStream, HintOpenError> {
        let authorization = self.authorization().map_err(|_| HintOpenError::Unauthorized)?;
        let client = stream_client().ok_or(HintOpenError::Unavailable)?;
        let url = format!("{}{HINTS_PATH}", self.base_url);
        let request = self
            .operation_headers(client.get(url))
            .header(header::AUTHORIZATION, authorization)
            .header(header::ACCEPT, "text/event-stream");
        tokio::time::timeout(OPEN_TIMEOUT, async {
            let response = request.send().await.map_err(|_| HintOpenError::Unavailable)?;
            Self::await_hello(response).await
        })
        .await
        .map_err(|_| HintOpenError::Unavailable)?
    }

    async fn await_hello(mut response: reqwest::Response) -> Result<HintStream, HintOpenError> {
        let status = response.status();
        if !status.is_success() {
            return Err(match status {
                StatusCode::UNAUTHORIZED => HintOpenError::Unauthorized,
                StatusCode::FORBIDDEN => HintOpenError::NotABot,
                StatusCode::TOO_MANY_REQUESTS => HintOpenError::TooMany,
                _ => HintOpenError::Unavailable,
            });
        }
        let is_stream = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream"));
        if !is_stream {
            // The envelope of an error before the stream starts; anything else is not ours.
            let body = response.chunk().await.ok().flatten().unwrap_or_default();
            let code = body
                .get(..body.len().min(MAX_EVENT_BYTES))
                .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
                .and_then(|envelope| envelope.get("code").and_then(Value::as_i64));
            return Err(code.map_or(HintOpenError::Unavailable, classify_code));
        }
        let mut parser = SseParser::default();
        let mut ready: VecDeque<Frame> = VecDeque::new();
        loop {
            if let Some(frame) = ready.pop_front() {
                return match frame.name.as_str() {
                    "hello" => Ok(HintStream {
                        response,
                        parser,
                        ready,
                    }),
                    "revoked" => Err(HintOpenError::Unauthorized),
                    _ => Err(HintOpenError::Unavailable),
                };
            }
            match response.chunk().await {
                Ok(Some(chunk)) => parser
                    .push(&chunk, &mut ready)
                    .map_err(|_| HintOpenError::Unavailable)?,
                Ok(None) | Err(_) => return Err(HintOpenError::Unavailable),
            }
        }
    }
}

/// One dispatched SSE event.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Frame {
    name: String,
}

impl Frame {
    fn event(&self) -> HintEvent {
        match self.name.as_str() {
            "task" => HintEvent::Task,
            "inbox" => HintEvent::Inbox,
            "issue" => HintEvent::Issue,
            "draft" => HintEvent::Draft,
            "tools" => HintEvent::Tools,
            "access_changed" => HintEvent::AccessChanged,
            "revoked" => HintEvent::Revoked,
            "lagged" => HintEvent::Lagged,
            "shutdown" => HintEvent::Shutdown,
            _ => HintEvent::Unknown,
        }
    }
}

/// An incremental SSE line parser. Only `event:` is read; payloads are never kept, because the
/// hub acts on event names alone.
#[derive(Default)]
struct SseParser {
    line: Vec<u8>,
    name: Option<String>,
    /// Whether a line of the current event has been seen (so an empty line dispatches).
    open: bool,
}

impl SseParser {
    fn push(&mut self, bytes: &[u8], out: &mut VecDeque<Frame>) -> Result<(), HintReadError> {
        for &byte in bytes {
            if byte == b'\n' {
                self.finish_line(out);
            } else {
                if self.line.len() >= MAX_EVENT_BYTES {
                    return Err(HintReadError::Oversized);
                }
                self.line.push(byte);
            }
        }
        Ok(())
    }

    fn finish_line(&mut self, out: &mut VecDeque<Frame>) {
        let raw = std::mem::take(&mut self.line);
        let line = raw.strip_suffix(b"\r").unwrap_or(&raw);
        if line.is_empty() {
            if self.open {
                out.push_back(Frame {
                    name: self.name.take().unwrap_or_else(|| "message".to_string()),
                });
            }
            self.open = false;
            self.name = None;
            return;
        }
        if line.first() == Some(&b':') {
            return;
        }
        self.open = true;
        if let Some(value) = line.strip_prefix(b"event:") {
            let value = value.strip_prefix(b" ").unwrap_or(value);
            self.name = std::str::from_utf8(value).ok().map(str::to_string);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(chunks: &[&[u8]]) -> Result<Vec<String>, HintReadError> {
        let mut parser = SseParser::default();
        let mut out = VecDeque::new();
        for chunk in chunks {
            parser.push(chunk, &mut out)?;
        }
        Ok(out.into_iter().map(|frame| frame.name).collect())
    }

    #[test]
    fn events_are_dispatched_on_the_empty_line_whatever_the_chunking() {
        let whole = feed(&[b"event: hello\ndata: {}\n\nevent: tools\ndata: {}\n\n"]);
        assert_eq!(whole, Ok(vec!["hello".to_string(), "tools".to_string()]));
        let split = feed(&[b"eve", b"nt: to", b"ols\nda", b"ta: {}\n", b"\n"]);
        assert_eq!(split, Ok(vec!["tools".to_string()]));
    }

    #[test]
    fn comments_and_carriage_returns_are_handled() {
        let events = feed(&[b": keep-alive\n\nevent: task\r\ndata: {\"id\":\"x\"}\r\n\r\n:\n\n"]);
        assert_eq!(events, Ok(vec!["task".to_string()]));
    }

    #[test]
    fn an_event_without_a_name_is_a_message_and_unknown_to_the_hub() {
        let events = feed(&[b"data: x\n\n"]);
        assert_eq!(events, Ok(vec!["message".to_string()]));
        let frame = Frame {
            name: "message".to_string(),
        };
        assert_eq!(frame.event(), HintEvent::Unknown);
    }

    #[test]
    fn a_line_longer_than_the_bound_ends_the_stream() {
        let long = vec![b'a'; MAX_EVENT_BYTES + 1];
        assert_eq!(feed(&[&long]), Err(HintReadError::Oversized));
        let fits = vec![b'a'; MAX_EVENT_BYTES];
        assert!(feed(&[&fits, b"\n"]).is_ok());
    }

    #[test]
    fn every_documented_event_maps() {
        let cases = [
            ("task", HintEvent::Task),
            ("inbox", HintEvent::Inbox),
            ("issue", HintEvent::Issue),
            ("draft", HintEvent::Draft),
            ("tools", HintEvent::Tools),
            ("access_changed", HintEvent::AccessChanged),
            ("revoked", HintEvent::Revoked),
            ("lagged", HintEvent::Lagged),
            ("shutdown", HintEvent::Shutdown),
            ("surprise", HintEvent::Unknown),
        ];
        for (name, event) in cases {
            let frame = Frame { name: name.to_string() };
            assert_eq!(frame.event(), event, "{name}");
        }
    }

    #[test]
    fn envelope_codes_map_like_statuses() {
        assert_eq!(classify_code(401), HintOpenError::Unauthorized);
        assert_eq!(classify_code(403), HintOpenError::NotABot);
        assert_eq!(classify_code(429), HintOpenError::TooMany);
        assert_eq!(classify_code(500), HintOpenError::Unavailable);
    }
}
