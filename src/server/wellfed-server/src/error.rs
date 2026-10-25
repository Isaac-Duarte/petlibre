//! Every failure as RFC 9457 `application/problem+json`, mapped in one
//! place from the service's errors and axum's rejections.

use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use wellfed_api::{ErrorCode, FeederRefusal, Problem, Settings};
use wellfed_service::Error;

/// A request's failure.
#[derive(Debug)]
pub(crate) enum ApiError {
    Service(Error),
    /// A failure the server itself decides, with a detail for people.
    Code(ErrorCode, Option<String>),
}

impl From<Error> for ApiError {
    fn from(error: Error) -> Self {
        Self::Service(error)
    }
}

impl From<JsonRejection> for ApiError {
    fn from(rejection: JsonRejection) -> Self {
        Self::Code(ErrorCode::Invalid, Some(rejection.body_text()))
    }
}

impl From<PathRejection> for ApiError {
    fn from(rejection: PathRejection) -> Self {
        Self::Code(ErrorCode::NotFound, Some(rejection.body_text()))
    }
}

impl From<QueryRejection> for ApiError {
    fn from(rejection: QueryRejection) -> Self {
        Self::Code(ErrorCode::Invalid, Some(rejection.body_text()))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (code, detail, refusal) = match self {
            Self::Code(code, detail) => (code, detail, None),
            Self::Service(Error::Stale(settings)) => return stale(settings),
            Self::Service(error) => service(&error),
        };
        let status = status(code);
        let problem = Problem {
            kind: "about:blank".into(),
            title: status.canonical_reason().unwrap_or("Error").into(),
            status: status.as_u16(),
            code,
            detail,
            refusal,
        };
        let mut response = (status, Json(problem)).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );
        response
    }
}

/// A stale edit gets `412` with the current resource, so the client can
/// show it and decide again.
fn stale(settings: Settings) -> Response {
    let mut response = (StatusCode::PRECONDITION_FAILED, Json(settings)).into_response();
    if let Ok(tag) = HeaderValue::from_str(&crate::routes::etag(settings.generation)) {
        response.headers_mut().insert(header::ETAG, tag);
    }
    response
}

fn service(error: &Error) -> (ErrorCode, Option<String>, Option<FeederRefusal>) {
    let detail = Some(error.to_string());
    match error {
        Error::SetupClosed => (ErrorCode::SetupClosed, detail, None),
        Error::BadCredentials => (ErrorCode::BadCredentials, detail, None),
        Error::Invalid(_) | Error::Invite(_) => (ErrorCode::Invalid, detail, None),
        Error::NotFound => (ErrorCode::NotFound, detail, None),
        Error::AlreadyAdopted => (ErrorCode::AlreadyAdopted, detail, None),
        Error::InviteExpired => (ErrorCode::InviteExpired, detail, None),
        Error::Unreachable(_) => (ErrorCode::Unreachable, detail, None),
        Error::Offline => (ErrorCode::FeederOffline, detail, None),
        Error::Unknown => (ErrorCode::OutcomeUnknown, detail, None),
        Error::Refused(refusal) => (ErrorCode::FeederRefused, detail, Some(*refusal)),
        // Handled before: it carries the current resource.
        Error::Stale(_) => (ErrorCode::Internal, None, None),
        Error::Store(_)
        | Error::Unexpected
        | Error::Undecodable(_)
        | Error::Random
        | Error::Hash(_)
        | Error::Unseal => {
            tracing::error!("{error}");
            (ErrorCode::Internal, None, None)
        }
    }
}

fn status(code: ErrorCode) -> StatusCode {
    match code {
        ErrorCode::SetupClosed | ErrorCode::AlreadyAdopted => StatusCode::CONFLICT,
        ErrorCode::Unauthenticated | ErrorCode::BadCredentials => StatusCode::UNAUTHORIZED,
        ErrorCode::Forbidden | ErrorCode::CrossOrigin => StatusCode::FORBIDDEN,
        ErrorCode::UnknownHost => StatusCode::MISDIRECTED_REQUEST,
        ErrorCode::NotFound => StatusCode::NOT_FOUND,
        ErrorCode::Invalid => StatusCode::BAD_REQUEST,
        ErrorCode::PreconditionRequired => StatusCode::PRECONDITION_REQUIRED,
        ErrorCode::InviteExpired => StatusCode::GONE,
        ErrorCode::Unreachable => StatusCode::BAD_GATEWAY,
        ErrorCode::FeederOffline => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::OutcomeUnknown => StatusCode::GATEWAY_TIMEOUT,
        // Every refusal is final for its key, `busy` included: the feeder
        // remembers a refused write under its id and answers a retry with the
        // same refusal. A later attempt is a new request under a new key.
        ErrorCode::FeederRefused => StatusCode::CONFLICT,
        ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A retry with the same key replays the refusal, so a busy feeder is
    /// not "try again" (503) but a refusal like any other.
    #[test]
    fn a_busy_refusal_is_final_for_its_key() {
        let response = ApiError::Service(Error::Refused(FeederRefusal::Busy)).into_response();

        assert_eq!(response.status(), StatusCode::CONFLICT);
    }
}
