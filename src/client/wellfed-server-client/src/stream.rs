//! Following `GET /api/v1/stream`, the server's Server-Sent Events.
//!
//! [`Parser`] reads the event-stream format without IO, and [`Updates`]
//! drives it over a response body. Each update carries the stream's last
//! event ID as of that update, as a browser's `EventSource` keeps it: a
//! client that reconnects sends it back as `Last-Event-ID` and gets what it
//! missed, or a `resync` whose empty id clears it.

use std::collections::VecDeque;

use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use wellfed_api::{StreamEvent, StreamQuery};
use reqwest::header::{self, HeaderValue};
use reqwest::{Method, StatusCode};

use crate::{Client, Error, expect_status};

/// One update from the stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    /// The last event ID: the position to resume after, from this block's
    /// `id` field or the latest before it. Empty when the client must start
    /// afresh, or before the server has named one.
    pub id: String,
    pub event: StreamEvent,
}

impl Client {
    /// Follow the event stream, with the sample metrics `query` names,
    /// resuming after `last_event_id` when given.
    pub async fn stream(
        &self,
        query: &StreamQuery,
        last_event_id: Option<&str>,
    ) -> Result<Updates, Error> {
        let mut request = self.api(Method::GET, "/stream").query(query).header(
            header::ACCEPT,
            HeaderValue::from_static("text/event-stream"),
        );
        if let Some(id) = last_event_id {
            request = request.header("Last-Event-ID", id);
        }

        let response = expect_status(request.send().await?, StatusCode::OK).await?;
        let body = response
            .bytes_stream()
            .map(|chunk| chunk.map(|bytes| bytes.to_vec()))
            .boxed();
        Ok(Updates {
            body,
            parser: Parser::default(),
            ready: VecDeque::new(),
        })
    }
}

/// An open event stream. It ends when the server ends it: at shutdown, or
/// once the session no longer holds.
pub struct Updates {
    body: BoxStream<'static, reqwest::Result<Vec<u8>>>,
    parser: Parser,
    ready: VecDeque<Block>,
}

impl std::fmt::Debug for Updates {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Updates")
            .field("ready", &self.ready.len())
            .finish_non_exhaustive()
    }
}

impl Updates {
    /// The next update, or `None` once the stream has ended. Cancel-safe:
    /// a block read but not yet returned stays queued.
    pub async fn next(&mut self) -> Option<Result<Update, Error>> {
        loop {
            if let Some(block) = self.ready.pop_front() {
                return Some(block.update());
            }

            match self.body.next().await? {
                Ok(chunk) => self.ready.extend(self.parser.push(&chunk)),
                Err(error) => return Some(Err(error.into())),
            }
        }
    }
}

/// One dispatched block: the last event ID and its joined `data` lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Block {
    id: String,
    data: String,
}

impl Block {
    fn update(self) -> Result<Update, Error> {
        Ok(Update {
            id: self.id,
            event: serde_json::from_str(&self.data)?,
        })
    }
}

/// The `text/event-stream` format, read incrementally: bytes in, blocks
/// out. A leading byte-order mark is dropped, and lines end in LF or CRLF
/// (the server never sends a bare CR). The last event ID persists across
/// blocks, so a block with only an `id` moves it; comments, `event` and
/// `retry` fields are skipped, and a block whose data is empty dispatches
/// nothing.
#[derive(Debug, Default)]
pub(crate) struct Parser {
    /// Whether the first line has begun, past any byte-order mark.
    started: bool,
    line: Vec<u8>,
    last_id: String,
    data: Option<String>,
}

impl Parser {
    /// Read `bytes`, returning the blocks they complete.
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Vec<Block> {
        let mut blocks = Vec::new();

        for &byte in bytes {
            if byte != b'\n' {
                self.line.push(byte);
                continue;
            }
            let mut line = std::mem::take(&mut self.line);
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if !self.started {
                self.started = true;
                if line.starts_with(BOM) {
                    line.drain(..BOM.len());
                }
            }
            if let Some(block) = self.line(&String::from_utf8_lossy(&line)) {
                blocks.push(block);
            }
        }

        blocks
    }

    fn line(&mut self, line: &str) -> Option<Block> {
        if line.is_empty() {
            let data = self.data.take().filter(|data| !data.is_empty())?;
            return Some(Block {
                id: self.last_id.clone(),
                data,
            });
        }

        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "data" => match &mut self.data {
                Some(data) => {
                    data.push('\n');
                    data.push_str(value);
                }
                None => self.data = Some(value.to_owned()),
            },
            "id" if !value.contains('\0') => value.clone_into(&mut self.last_id),
            // A comment (empty field), `event`, `retry` or unknown.
            _ => {}
        }
        None
    }
}

/// UTF-8's byte-order mark, which may open a stream.
const BOM: &[u8] = b"\xef\xbb\xbf";

#[cfg(test)]
mod tests {
    use super::*;

    fn block(id: &str, data: &str) -> Block {
        Block {
            id: id.into(),
            data: data.into(),
        }
    }

    #[test]
    fn blocks_dispatch_on_a_blank_line_with_their_fields_in_any_order() {
        let mut parser = Parser::default();
        let text = "\u{feff}data: {\"type\":\"resync\"}\nid:\n\n: keep-alive\n\nid: 7-1\r\ndata:a\r\ndata: b\r\n\r\n";
        assert_eq!(
            parser.push(text.as_bytes()),
            [block("", "{\"type\":\"resync\"}"), block("7-1", "a\nb"),]
        );
    }

    #[test]
    fn a_block_split_across_chunks_waits_for_its_end() {
        let mut parser = Parser::default();
        assert_eq!(parser.push(b"id: 1-2\nda"), []);
        assert_eq!(parser.push(b"ta: x\n"), []);
        assert_eq!(parser.push(b"\n"), [block("1-2", "x")]);
    }

    #[test]
    fn the_last_event_id_persists_and_empty_data_dispatches_nothing() {
        let mut parser = Parser::default();
        assert_eq!(parser.push(b"data: y\n\n"), [block("", "y")], "none yet");
        assert_eq!(parser.push(b"id: 3\n\n"), [], "no data, no block");
        assert_eq!(parser.push(b"data:\n\n"), [], "empty data, no block");
        assert_eq!(parser.push(b"data: z\n\n"), [block("3", "z")]);
        assert_eq!(parser.push(b"id\ndata: w\n\n"), [block("", "w")]);
    }

    #[test]
    fn a_block_decodes_to_its_update() {
        let resync = block("", "{\"type\":\"resync\"}").update();
        assert_eq!(
            resync.ok(),
            Some(Update {
                id: String::new(),
                event: StreamEvent::Resync,
            })
        );
        assert!(matches!(
            block("", "nonsense").update(),
            Err(Error::Decode(_))
        ));
    }
}
