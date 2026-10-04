//! Stable admission refusal codes. HTTP dispatch and refusal precedence are backend-owned.
/// Error codes of one router, each prefixed with its family name.
pub struct Codes {
    /// 415: the body is not uncompressed JSON.
    pub content_type: &'static str,
    /// 401: a required bearer credential is missing or malformed.
    pub credential: &'static str,
    /// 413: the body exceeds its limit.
    pub limit: &'static str,
    /// 400: the body does not parse as a request.
    pub malformed: &'static str,
    /// 400: the operation refused a well-formed request.
    pub refused: &'static str,
    /// 403: the credential may not perform the operation.
    pub unauthorized: &'static str,
    /// 408: the body or the operation did not finish in time.
    pub timeout: &'static str,
    /// 500: storage failed or the reply exceeded its limit.
    pub server_error: &'static str,
    /// 503: every permit is taken; retry after `Retry-After`.
    pub busy: &'static str,
}

/// The [`Codes`] of a router family, e.g. `codes!("enrollment")`.
#[macro_export]
macro_rules! refusal_codes {
    ($family:literal) => {
        $crate::refusal::Codes {
            content_type: concat!($family, "-content-type"),
            credential: concat!($family, "-credential"),
            limit: concat!($family, "-limit"),
            malformed: concat!($family, "-malformed"),
            refused: concat!($family, "-refused"),
            unauthorized: concat!($family, "-unauthorized"),
            timeout: concat!($family, "-timeout"),
            server_error: concat!($family, "-server-error"),
            busy: concat!($family, "-busy"),
        }
    };
}

/// Endpoint-specific client interpretation, independent of HTTP execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bootstrap {
    Claimed,
    SetupRejected,
    SetupExpired,
    Quota,
    Unknown,
}

impl Bootstrap {
    pub fn classify(code: Option<&str>, claim: bool) -> Self {
        match code {
            Some("bootstrap-storage-already-claimed") => Self::Claimed,
            Some("bootstrap-setup-invitation-rejected") if claim => Self::SetupRejected,
            Some("bootstrap-setup-invitation-expired") if claim => Self::SetupExpired,
            Some("attachment-quota-exceeded") => Self::Quota,
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Enrollment {
    Stale,
    Busy,
    Unauthorized,
    Timeout,
    Refused,
    Server,
}

impl Enrollment {
    pub fn classify(status: u16, code: Option<&str>) -> Self {
        match code {
            Some("membership-stale") => Self::Stale,
            Some("enrollment-busy") => Self::Busy,
            Some("enrollment-unauthorized") => Self::Unauthorized,
            Some("enrollment-timeout") => Self::Timeout,
            Some(_) if (400..500).contains(&status) => Self::Refused,
            _ => Self::Server,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tail {
    Malformed,
    BatchKnown,
    Stale,
    Unauthorized,
    PrefixIdentityCollision,
    Quota,
    Unknown,
}

impl Tail {
    pub fn classify(code: Option<&str>) -> Self {
        match code {
            Some("encrypted-tail-malformed") => Self::Malformed,
            Some("encrypted-tail-batch-known") => Self::BatchKnown,
            Some("membership-stale") => Self::Stale,
            Some("encrypted-tail-unauthorized") => Self::Unauthorized,
            Some("encrypted-tail-prefix-identity-collision") => Self::PrefixIdentityCollision,
            Some("attachment-quota-exceeded") => Self::Quota,
            _ => Self::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_refusals_preserve_access_and_outcome_uncertainty() {
        assert_eq!(
            Bootstrap::classify(Some("bootstrap-setup-invitation-expired"), true),
            Bootstrap::SetupExpired
        );
        assert_eq!(
            Bootstrap::classify(Some("bootstrap-setup-invitation-expired"), false),
            Bootstrap::Unknown
        );
        assert_eq!(
            Enrollment::classify(401, Some("enrollment-credential")),
            Enrollment::Refused
        );
        assert_eq!(
            Enrollment::classify(503, Some("enrollment-busy")),
            Enrollment::Busy
        );
        assert_eq!(
            Enrollment::classify(403, Some("enrollment-unauthorized")),
            Enrollment::Unauthorized
        );
        assert_eq!(Enrollment::classify(403, None), Enrollment::Server);
        assert_eq!(
            Tail::classify(Some("encrypted-tail-credential")),
            Tail::Unknown
        );
        assert_eq!(
            Tail::classify(Some("encrypted-image-unauthorized")),
            Tail::Unknown
        );
        assert_eq!(
            Tail::classify(Some("encrypted-tail-unauthorized")),
            Tail::Unauthorized
        );
        assert_eq!(
            Tail::classify(Some("attachment-quota-exceeded")),
            Tail::Quota
        );
    }
}
