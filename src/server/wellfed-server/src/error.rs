//! Every failure as RFC 9457 `application/problem+json`: the service's
//! errors with the code each domain gives its refusals, and axum's
//! rejections.

use axum::Json;
use axum::extract::rejection::{BytesRejection, JsonRejection, PathRejection, QueryRejection};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use wellfed_api::{ErrorCode, FeederRefusal, Generation, Problem};
use wellfed_service::{AccountError, Error, Reply};

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

impl From<BytesRejection> for ApiError {
    fn from(rejection: BytesRejection) -> Self {
        let code = if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ErrorCode::TooLarge
        } else {
            ErrorCode::Invalid
        };
        Self::Code(code, Some(rejection.body_text()))
    }
}

impl From<QueryRejection> for ApiError {
    fn from(rejection: QueryRejection) -> Self {
        Self::Code(ErrorCode::Invalid, Some(rejection.body_text()))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let release = match &self {
            Self::Service(Error::Release(release)) => release.refusal(),
            _ => None,
        };
        let retry_after = match &self {
            Self::Service(Error::Account(AccountError::Limited { retry_after })) => {
                Some(retry_after.as_secs())
            }
            _ => None,
        };
        let (code, detail, refusal) = match self {
            Self::Code(code, detail) => (code, detail, None),
            Self::Service(Error::Stale(settings)) => return stale(&settings, settings.generation),
            Self::Service(Error::StalePlans(plans)) => return stale(&plans, plans.generation),
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
            release,
        };
        let mut response = (status, Json(problem)).into_response();
        let headers = response.headers_mut();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );
        if let Some(seconds) = retry_after {
            headers.insert(header::RETRY_AFTER, HeaderValue::from(seconds));
        }
        response
    }
}

/// A stale edit gets `412` with the current resource (the settings or the
/// plans), so the client can show it and decide again.
fn stale(current: &impl Serialize, generation: Generation) -> Response {
    let mut response = (StatusCode::PRECONDITION_FAILED, Json(current)).into_response();
    if let Ok(tag) = HeaderValue::from_str(&crate::routes::feeders::etag(generation)) {
        response.headers_mut().insert(header::ETAG, tag);
    }
    response
}

/// The service decides each refusal's code ([`Error::reply`]); this adds
/// what HTTP needs: the detail, the feeder's refusal, and the log.
fn service(error: &Error) -> (ErrorCode, Option<String>, Option<FeederRefusal>) {
    match error.reply() {
        Reply::Code(code) => (code, Some(error.to_string()), error.refusal()),
        Reply::Warn(code) => {
            tracing::warn!("{}", Chain(error));
            (code, Some(error.to_string()), error.refusal())
        }
        Reply::Internal => {
            tracing::error!("{}", Chain(error));
            (ErrorCode::Internal, None, None)
        }
    }
}

/// An error and each of its sources, for the log.
struct Chain<'a>(&'a dyn std::error::Error);

impl std::fmt::Display for Chain<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)?;
        let mut source = self.0.source();
        while let Some(error) = source {
            write!(f, ": {error}")?;
            source = error.source();
        }
        Ok(())
    }
}

fn status(code: ErrorCode) -> StatusCode {
    match code {
        ErrorCode::SetupClosed
        | ErrorCode::AlreadyAdopted
        | ErrorCode::LastOwner
        | ErrorCode::NameTaken
        | ErrorCode::FoodInUse
        | ErrorCode::NoCurrentFood
        | ErrorCode::FeederArchived
        | ErrorCode::ClientKeyTaken => StatusCode::CONFLICT,
        ErrorCode::Unauthenticated | ErrorCode::BadCredentials => StatusCode::UNAUTHORIZED,
        ErrorCode::LoginLimited => StatusCode::TOO_MANY_REQUESTS,
        ErrorCode::Forbidden
        | ErrorCode::SessionRequired
        | ErrorCode::CrossOrigin
        | ErrorCode::AccountDisabled
        | ErrorCode::TokenRequired
        | ErrorCode::DirectKeyRefused => StatusCode::FORBIDDEN,
        ErrorCode::ZoneUnsupported => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::UnknownHost => StatusCode::MISDIRECTED_REQUEST,
        ErrorCode::NotFound => StatusCode::NOT_FOUND,
        ErrorCode::Invalid => StatusCode::BAD_REQUEST,
        ErrorCode::PreconditionRequired => StatusCode::PRECONDITION_REQUIRED,
        ErrorCode::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        ErrorCode::InviteExpired => StatusCode::GONE,
        ErrorCode::Unreachable => StatusCode::BAD_GATEWAY,
        ErrorCode::FeederOffline => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::OutcomeUnknown => StatusCode::GATEWAY_TIMEOUT,
        ErrorCode::KeyReused => StatusCode::UNPROCESSABLE_ENTITY,
        // Every refusal is final for its key, `busy` included: the feeder
        // replays a refused write's answer while it remembers the id, so a
        // later attempt is a new request under a new key.
        ErrorCode::FeederRefused => StatusCode::CONFLICT,
        ErrorCode::ReleaseRefused => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::FetchDisabled => StatusCode::CONFLICT,
        ErrorCode::ReleasesUnreachable => StatusCode::BAD_GATEWAY,
        ErrorCode::InstallRunning | ErrorCode::StockDiffers | ErrorCode::StockBackupNeeded => {
            StatusCode::CONFLICT
        }
        ErrorCode::NoStockBackup => StatusCode::NOT_FOUND,
        ErrorCode::StockRefused | ErrorCode::RestoreMismatch => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::ImportClosed | ErrorCode::ExportRunning => StatusCode::CONFLICT,
        ErrorCode::ArchiveInvalid => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use wellfed_service::ZoneError;

    use super::*;

    /// A retry with the same key replays the refusal, so a busy feeder is
    /// not "try again" (503) but a refusal like any other.
    #[test]
    fn a_busy_refusal_is_final_for_its_key() {
        let refused = Error::Refused(FeederRefusal::Busy);

        let response = ApiError::Service(refused).into_response();

        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    /// A real zone no POSIX rule describes is `422`: the request was well
    /// formed, but a feeder could not keep the zone.
    #[test]
    fn a_zone_no_rule_describes_is_unprocessable() {
        let irregular = ZoneError::Irregular {
            name: "Asia/Gaza".into(),
            rule: "EET-2EEST".into(),
            at: jiff::Timestamp::UNIX_EPOCH,
        };

        let response = ApiError::Service(Error::Zone(irregular)).into_response();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
