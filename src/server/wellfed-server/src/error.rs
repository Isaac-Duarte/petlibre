//! Every failure as RFC 9457 `application/problem+json`, mapped in one
//! place from the service's errors and axum's rejections.

use axum::Json;
use axum::extract::rejection::{BytesRejection, JsonRejection, PathRejection, QueryRejection};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use wellfed_api::{ErrorCode, FeederRefusal, Problem, Settings};
use wellfed_service::{
    AccountError, CommandError, Error, FeederError, HouseholdError, InstallError, OwnerDataError,
    ReleaseError, TokenError, ZoneError,
};

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
        let (code, detail, refusal) = match self {
            Self::Code(code, detail) => (code, detail, None),
            Self::Service(Error::Command(CommandError::Stale(settings))) => return stale(settings),
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
    if let Ok(tag) = HeaderValue::from_str(&crate::routes::feeders::etag(settings.generation)) {
        response.headers_mut().insert(header::ETAG, tag);
    }
    response
}

fn service(error: &Error) -> (ErrorCode, Option<String>, Option<FeederRefusal>) {
    let detail = Some(error.to_string());
    let code = match error {
        Error::Invalid(_) => ErrorCode::Invalid,
        Error::Account(account) => match account {
            AccountError::SetupClosed => ErrorCode::SetupClosed,
            AccountError::BadCredentials => ErrorCode::BadCredentials,
            AccountError::Disabled => ErrorCode::AccountDisabled,
            AccountError::NotFound => ErrorCode::NotFound,
            AccountError::NameTaken => ErrorCode::NameTaken,
            AccountError::LastOwner => ErrorCode::LastOwner,
            // Only `reset-owner`, on the host, picks an owner by name.
            AccountError::NotAnOwner(_) | AccountError::NoOwner | AccountError::OwnerAmbiguous => {
                ErrorCode::Invalid
            }
            AccountError::Hash(_) | AccountError::Hashing | AccountError::Expiry => {
                return internal(error);
            }
        },
        Error::Token(token) => match token {
            TokenError::NotFound => ErrorCode::NotFound,
            TokenError::SessionRequired => ErrorCode::SessionRequired,
        },
        Error::Household(household) => match household {
            HouseholdError::NotSetUp => ErrorCode::NotFound,
            HouseholdError::Zone(ZoneError::Unknown(_)) => ErrorCode::Invalid,
            HouseholdError::Zone(
                ZoneError::NoRule(_) | ZoneError::Unsupported { .. } | ZoneError::Irregular { .. },
            ) => ErrorCode::ZoneUnsupported,
        },
        Error::Feeder(feeder) => match feeder {
            FeederError::NotFound => ErrorCode::NotFound,
            FeederError::AlreadyAdopted => ErrorCode::AlreadyAdopted,
            FeederError::Invite(_) => ErrorCode::Invalid,
            FeederError::InviteExpired => ErrorCode::InviteExpired,
            FeederError::Unreachable(_) => ErrorCode::Unreachable,
            FeederError::Refused(refusal) => {
                return (ErrorCode::FeederRefused, detail, Some(*refusal));
            }
            FeederError::Lookup
            | FeederError::Undecodable(_)
            | FeederError::UndecodableSample
            | FeederError::Unseal => {
                return internal(error);
            }
        },
        Error::OwnerData(owner_data) => match owner_data {
            OwnerDataError::PetNotFound
            | OwnerDataError::WeightNotFound
            | OwnerDataError::NotAssigned
            | OwnerDataError::FoodNotFound
            | OwnerDataError::RefillNotFound => ErrorCode::NotFound,
            OwnerDataError::FoodInUse => ErrorCode::FoodInUse,
            OwnerDataError::NoCurrentFood => ErrorCode::NoCurrentFood,
        },
        Error::Command(command) => match command {
            CommandError::Offline => ErrorCode::FeederOffline,
            CommandError::Unknown => ErrorCode::OutcomeUnknown,
            CommandError::Refused(refusal) => {
                return (ErrorCode::FeederRefused, detail, Some(*refusal));
            }
            // Handled before: it carries the current resource.
            CommandError::Stale(_) => return (ErrorCode::Internal, None, None),
            CommandError::KeyReused => ErrorCode::KeyReused,
            CommandError::NoEntry => ErrorCode::NotFound,
            CommandError::Unexpected | CommandError::Body(_) => return internal(error),
        },
        Error::Install(install) => match install {
            InstallError::Running => ErrorCode::InstallRunning,
            InstallError::None => ErrorCode::NotFound,
        },
        Error::Release(release) => match release {
            ReleaseError::Refused(_) => ErrorCode::ReleaseRefused,
            ReleaseError::FetchDisabled => ErrorCode::FetchDisabled,
            ReleaseError::Unreachable(_)
            | ReleaseError::Listing(_)
            | ReleaseError::ListingTooLarge => {
                // The cause (a status, a TLS failure) is only in the chain.
                tracing::warn!("{}", Chain(release));
                ErrorCode::ReleasesUnreachable
            }
            ReleaseError::NotFound => ErrorCode::NotFound,
            ReleaseError::Cursor | ReleaseError::Limit { .. } => ErrorCode::Invalid,
            ReleaseError::Changed
            | ReleaseError::Client(_)
            | ReleaseError::Cache { .. }
            | ReleaseError::Entry { .. }
            | ReleaseError::Task => {
                tracing::error!("{}", Chain(release));
                return (ErrorCode::Internal, None, None);
            }
        },
        Error::Backup(backup) => {
            // The cause (a full disk, a permission) is only in the chain.
            tracing::error!("{}", Chain(backup));
            return (ErrorCode::Internal, None, None);
        }
        Error::Store(_) | Error::Random(_) => return internal(error),
    };
    (code, detail, None)
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

/// A failure of the server's own: logged, and answered without its detail.
fn internal(error: &Error) -> (ErrorCode, Option<String>, Option<FeederRefusal>) {
    tracing::error!("{error}");
    (ErrorCode::Internal, None, None)
}

fn status(code: ErrorCode) -> StatusCode {
    match code {
        ErrorCode::SetupClosed
        | ErrorCode::AlreadyAdopted
        | ErrorCode::LastOwner
        | ErrorCode::NameTaken
        | ErrorCode::FoodInUse
        | ErrorCode::NoCurrentFood => StatusCode::CONFLICT,
        ErrorCode::Unauthenticated | ErrorCode::BadCredentials => StatusCode::UNAUTHORIZED,
        ErrorCode::Forbidden
        | ErrorCode::SessionRequired
        | ErrorCode::CrossOrigin
        | ErrorCode::AccountDisabled => StatusCode::FORBIDDEN,
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
        ErrorCode::InstallRunning => StatusCode::CONFLICT,
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
        let refused = CommandError::Refused(FeederRefusal::Busy);

        let response = ApiError::Service(refused.into()).into_response();

        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    /// A real zone no POSIX rule describes is `422 zone_unsupported`; a
    /// name the tzdb does not know is a plain `400 invalid`.
    #[test]
    fn a_zone_is_refused_by_kind() {
        let irregular = ZoneError::Irregular {
            name: "Asia/Gaza".into(),
            rule: "EET-2EEST".into(),
            at: jiff::Timestamp::UNIX_EPOCH,
        };
        let unknown = ZoneError::Unknown("Mars/Olympus".into());

        let code = |zone| service(&HouseholdError::Zone(zone).into());
        let (code_irregular, detail, _) = code(irregular);
        assert_eq!(code_irregular, ErrorCode::ZoneUnsupported);
        assert!(detail.is_some_and(|detail| detail.contains("Asia/Gaza")));
        assert_eq!(
            status(ErrorCode::ZoneUnsupported),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(code(unknown).0, ErrorCode::Invalid);
    }
}
