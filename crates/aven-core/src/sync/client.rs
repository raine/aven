//! The end-to-end encrypted sync client shared by every host.
//!
//! Hosts run [`engine`] operations as sans-IO [`Session`]s, persist
//! protected keys through [`keys::ProtectedStorage`] and supply policy,
//! storage and paths through [`ClientHost`].
pub mod bootstrap;
pub mod coordination;
mod device_label;
pub mod engine;
pub mod enrollment;
pub mod errors;
mod exchange;
mod host;
pub mod invitation;
pub mod keys;
mod origin;
pub mod tail;

pub use device_label::clean_label;
pub use exchange::{
    HttpHeader, HttpResponse, Link, PreparedRequest, RequestContext, Session, Step,
};
pub use host::ClientHost;
pub use invitation::{DeviceInvitation, InvitationCheck, SetupInvitation};
pub use origin::{MAX_SERVER_BYTES, server_origin};
