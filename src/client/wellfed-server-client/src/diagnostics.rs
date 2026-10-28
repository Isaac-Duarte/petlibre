//! A feed's trace, the latest statistics, and following a feeder's debug
//! log (`GET /feeders/{id}/log`, Server-Sent Events).

use std::collections::VecDeque;

use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use reqwest::header::{self, HeaderValue};
use reqwest::{Method, StatusCode};
use wellfed_api::{FeedTrace, FeederId, FeederStats, LogEvent, TraceQuery};

use crate::stream::{Block, Parser};
use crate::{Client, Error, decode, expect_status};

impl Client {
    /// Feed `feed`'s trace, as the server fetched it after the feed.
    pub async fn feed_trace(
        &self,
        id: FeederId,
        feed: u32,
        query: &TraceQuery,
    ) -> Result<FeedTrace, Error> {
        let response = self
            .api(Method::GET, &format!("/feeders/{id}/feeds/{feed}/trace"))
            .query(query)
            .send()
            .await?;
        decode(response, StatusCode::OK).await
    }

    /// The latest read of each statistics section.
    pub async fn feeder_stats(&self, id: FeederId) -> Result<FeederStats, Error> {
        let response = self
            .api(Method::GET, &format!("/feeders/{id}/stats"))
            .send()
            .await?;
        decode(response, StatusCode::OK).await
    }

    /// Follow the feeder's debug log. Owners only.
    pub async fn follow_log(&self, id: FeederId) -> Result<LogBatches, Error> {
        let request = self.api(Method::GET, &format!("/feeders/{id}/log")).header(
            header::ACCEPT,
            HeaderValue::from_static("text/event-stream"),
        );

        let response = expect_status(request.send().await?, StatusCode::OK).await?;
        let body = response
            .bytes_stream()
            .map(|chunk| chunk.map(|bytes| bytes.to_vec()))
            .boxed();
        Ok(LogBatches {
            body,
            parser: Parser::default(),
            ready: VecDeque::new(),
        })
    }
}

/// A followed debug log. It ends when the feeder's tail does, at the
/// server's shutdown, or once the session no longer holds.
pub struct LogBatches {
    body: BoxStream<'static, reqwest::Result<Vec<u8>>>,
    parser: Parser,
    ready: VecDeque<Block>,
}

impl std::fmt::Debug for LogBatches {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogBatches")
            .field("ready", &self.ready.len())
            .finish_non_exhaustive()
    }
}

impl LogBatches {
    /// The next message, or `None` once the log has ended. Cancel-safe: a
    /// block read but not yet returned stays queued.
    pub async fn next(&mut self) -> Option<Result<LogEvent, Error>> {
        loop {
            if let Some(block) = self.ready.pop_front() {
                return Some(block.json());
            }

            match self.body.next().await? {
                Ok(chunk) => self.ready.extend(self.parser.push(&chunk)),
                Err(error) => return Some(Err(error.into())),
            }
        }
    }
}
