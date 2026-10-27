//! Every way a call to the server fails.

use wellfed_api::{ErrorCode, Problem, Settings};
use reqwest::{StatusCode, Url};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the server's address must be an http or https URL with a host, not {0}")]
    Base(Url),
    #[error("could not build the HTTP client")]
    Build(#[source] reqwest::Error),
    /// The request failed on the way, or its answer was cut off. A keyed
    /// write may have run: retry it under the same key, never a new one.
    #[error("the request to the server failed")]
    Http(#[from] reqwest::Error),
    /// The server refused, with its typed reason.
    #[error("the server answered {status}: {}", problem.title)]
    Problem {
        status: StatusCode,
        problem: Box<Problem>,
    },
    /// An edit named a generation the settings have moved past; these are
    /// the current settings.
    #[error("the settings moved on to generation {}", .0.generation)]
    Stale(Box<Settings>),
    #[error("the server answered {status} where {expected} was expected")]
    Unexpected {
        status: StatusCode,
        expected: StatusCode,
    },
    #[error("the server's answer does not decode")]
    Decode(#[from] serde_json::Error),
    #[error("the server's login answer set no session cookie")]
    NoSession,
}

impl Error {
    /// The problem's code, when the server refused with one.
    pub fn code(&self) -> Option<ErrorCode> {
        match self {
            Self::Problem { problem, .. } => Some(problem.code),
            _ => None,
        }
    }
}
