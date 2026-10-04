//! Enrollment transport. Verified evidence and active client inputs stay with their owner.
use crate::claim::Secret;
use serde::{Deserialize, Serialize};
/// Explicit current-head request context. Historical outcomes cannot authenticate it.
pub struct Authentication<'a> {
    pub vault: [u8; 32],
    pub genesis: [u8; 32],
    pub device: [u8; 32],
    pub head: [u8; 32],
    pub bearer: &'a Secret,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Operation {
    Register {
        context: Context,
        #[serde(with = "crate::base64_bytes")]
        declaration: Vec<u8>,
    },
    Post {
        vault: [u8; 32],
        handle: [u8; 32],
        #[serde(with = "crate::base64_bytes")]
        request: Vec<u8>,
    },
    Mailbox {
        vault: [u8; 32],
        handle: [u8; 32],
    },
    Admit {
        context: Context,
        handle: [u8; 32],
        #[serde(with = "crate::base64_bytes")]
        record: Vec<u8>,
    },
    Membership {
        context: Context,
    },
    PrepareManagement {
        context: Context,
    },
    Manage {
        context: Context,
        #[serde(with = "crate::base64_bytes")]
        record: Vec<u8>,
    },
    Cancel {
        context: Context,
        handle: [u8; 32],
    },
    Published {
        context: Context,
        descriptor: [u8; 32],
        component: Option<super::bootstrap::Component>,
        index: u64,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Reply<Evidence, Mailbox, ManagementPreparation> {
    Done,
    Registered(RegistrationStatus),
    Mailbox(Mailbox),
    Admitted(#[serde(with = "crate::base64_bytes")] Vec<u8>),
    Membership(Evidence),
    PreparedManagement(ManagementPreparation),
    Managed(#[serde(with = "crate::base64_bytes")] Vec<u8>),
    Cancelled(CancelStatus),
    Published(#[serde(with = "crate::base64_bytes")] Vec<u8>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RegistrationStatus {
    Open,
    Expired,
    Consumed,
}

/// Serialized outcome of cancelling a registered invitation. `Admitted` is a
/// hint only; callers resolve admission from the verified membership chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CancelStatus {
    Admitted,
    Cancelled,
}

pub const MAX_RECORD_BYTES: usize = 32768;
pub const PATH: &str = "/e2ee/enrollment/v1";
pub const CONTROL_LIMIT: usize = crate::base64_bytes::encoded_len(MAX_RECORD_BYTES) + 4096;
pub const PUBLISHED_RESPONSE_LIMIT: usize =
    crate::base64_bytes::encoded_len(super::bootstrap::MAX_REQUEST_BYTES) + 4096;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Context {
    pub vault: [u8; 32],
    pub genesis: [u8; 32],
    pub device: [u8; 32],
    pub head: [u8; 32],
}
impl Context {
    /// Converts active client inputs without coupling the wire format to their storage.
    pub fn active<'a, T>(inputs: &'a T) -> Self
    where
        &'a T: Into<Self>,
    {
        inputs.into()
    }
    pub fn auth<'a>(&self, bearer: &'a Secret) -> Authentication<'a> {
        Authentication {
            vault: self.vault,
            genesis: self.genesis,
            device: self.device,
            head: self.head,
            bearer,
        }
    }
}
