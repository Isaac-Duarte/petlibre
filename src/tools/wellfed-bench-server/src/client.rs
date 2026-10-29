//! The clients' side of the socket: `wellfed-bench`'s commands post a job,
//! follow its stream and exit with its code; `status` reads the state.
//!
//! Each request opens its own connection, so a cancel can go out while a
//! stream is being read. Leaving a stream (a second Ctrl+C, a closed
//! terminal) only detaches: the job runs on, and `attach` follows it again
//! from its first kept line.

use std::path::{Path, PathBuf};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde::de::DeserializeOwned;
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use wellfed_formats::bench::live::{BenchState, JobEnd, JobEvent, JobId};

use crate::api::{Accepted, Refusal, SOCKET};
use crate::jobs::JobSpec;

/// Why a client request did not get its answer.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Nothing listens on the socket: no server runs for this checkout.
    #[error(
        "No bench server answers on {}: start `wellfed-bench serve` in another terminal \
         (without it, only `status` and `status --break-lease` run)",
        socket.display()
    )]
    NoServer { socket: PathBuf },
    #[error("{}: {error}", socket.display())]
    Connect {
        socket: PathBuf,
        error: std::io::Error,
    },
    #[error("The bench server: {0}")]
    Http(#[from] hyper::Error),
    #[error("The bench server's request: {0}")]
    Request(#[from] hyper::http::Error),
    /// The server refused, with its reason.
    #[error(transparent)]
    Refused(#[from] Refusal),
    /// An answer that is neither what was asked for nor a refusal.
    #[error("The bench server answered {status}: {body}")]
    Unexpected { status: StatusCode, body: String },
    #[error("The bench server's answer: {0}")]
    Json(#[from] serde_json::Error),
    #[error("The bench server closed job {id}'s stream before it ended")]
    StreamCut { id: JobId },
}

/// How following a job ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Followed {
    Ended(JobEnd),
    /// The client stopped reading; the job runs on.
    Detached,
}

/// What a client does on the operator's interrupt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interrupt {
    /// Ask the server to cancel the job, and keep following its cleanup.
    Cancel,
    /// Stop following; the job runs on.
    Detach,
}

pub struct Client {
    socket: PathBuf,
}

impl Client {
    /// The server for the checkout at `root`.
    pub fn new(root: &Path) -> Self {
        Self::at(root.join(SOCKET))
    }

    pub fn at(socket: PathBuf) -> Self {
        Self { socket }
    }

    pub async fn state(&self) -> Result<BenchState, ClientError> {
        let response = self.send(Method::GET, "/state", None).await?;
        answer(response, StatusCode::OK).await
    }

    pub async fn submit(&self, spec: &JobSpec) -> Result<JobId, ClientError> {
        let body = serde_json::to_vec(spec)?;
        let response = self.send(Method::POST, "/jobs", Some(body)).await?;
        let accepted: Accepted = answer(response, StatusCode::CREATED).await?;
        Ok(accepted.id)
    }

    pub async fn cancel(&self, id: JobId) -> Result<(), ClientError> {
        let path = format!("/jobs/{id}/cancel");
        let response = self.send(Method::POST, &path, None).await?;
        let _: Accepted = answer(response, StatusCode::ACCEPTED).await?;
        Ok(())
    }

    /// Follow job `id` from its first kept line, handing each line to
    /// `line`, until it ends or an interrupt says to detach. A cancel is
    /// sent at once and the job's cleanup still followed; `interrupts`
    /// closing detaches too.
    pub async fn follow(
        &self,
        id: JobId,
        mut line: impl FnMut(&str),
        interrupts: &mut mpsc::UnboundedReceiver<Interrupt>,
    ) -> Result<Followed, ClientError> {
        let path = format!("/jobs/{id}/stream");
        let response = self.send(Method::GET, &path, None).await?;
        if response.status() != StatusCode::OK {
            return Err(refusal(response).await);
        }

        let mut body = response.into_body();
        let mut pending = Vec::new();
        loop {
            let frame = tokio::select! {
                frame = body.frame() => frame,
                interrupt = interrupts.recv() => match interrupt {
                    Some(Interrupt::Cancel) => {
                        // A job that ended as the cancel went out still
                        // sends its end on this stream.
                        match self.cancel(id).await {
                            Ok(()) | Err(ClientError::Refused(Refusal::Finished { .. })) => {}
                            Err(error) => return Err(error),
                        }
                        continue;
                    }
                    Some(Interrupt::Detach) | None => return Ok(Followed::Detached),
                },
            };
            let Some(frame) = frame else {
                return Err(ClientError::StreamCut { id });
            };
            let Ok(data) = frame?.into_data() else {
                continue;
            };

            pending.extend_from_slice(&data);
            while let Some(at) = pending.iter().position(|byte| *byte == b'\n') {
                let event: JobEvent = serde_json::from_slice(&pending[..at])?;
                pending.drain(..=at);
                match event {
                    JobEvent::Line(text) => line(&text),
                    JobEvent::End(end) => return Ok(Followed::Ended(end)),
                }
            }
        }
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<Vec<u8>>,
    ) -> Result<Response<Incoming>, ClientError> {
        let stream =
            UnixStream::connect(&self.socket)
                .await
                .map_err(|error| match error.kind() {
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
                        ClientError::NoServer {
                            socket: self.socket.clone(),
                        }
                    }
                    _ => ClientError::Connect {
                        socket: self.socket.clone(),
                        error,
                    },
                })?;
        let (mut sender, connection) =
            hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
        tokio::spawn(connection);

        let request = Request::builder()
            .method(method)
            .uri(path)
            .header(hyper::header::HOST, "bench")
            .header(hyper::header::CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from(body.unwrap_or_default())))?;
        Ok(sender.send_request(request).await?)
    }
}

/// The body of an answer with status `want`, or the refusal it carries.
async fn answer<T: DeserializeOwned>(
    response: Response<Incoming>,
    want: StatusCode,
) -> Result<T, ClientError> {
    if response.status() != want {
        return Err(refusal(response).await);
    }
    let body = response.into_body().collect().await?.to_bytes();
    Ok(serde_json::from_slice(&body)?)
}

async fn refusal(response: Response<Incoming>) -> ClientError {
    let status = response.status();
    let body = match response.into_body().collect().await {
        Ok(body) => body.to_bytes(),
        Err(error) => return error.into(),
    };
    match serde_json::from_slice::<Refusal>(&body) {
        Ok(refusal) => refusal.into(),
        Err(_) => ClientError::Unexpected {
            status,
            body: String::from_utf8_lossy(&body).into_owned(),
        },
    }
}
