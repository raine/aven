//! The end-to-end encrypted sync client shared by every host.
//!
//! Protocol clients run as sans-IO [`Session`]s that the host drives.
pub mod bootstrap;
pub mod enrollment;
mod exchange;
pub mod invitation;
pub mod keys;
mod origin;
pub mod tail;

pub use exchange::{
    HttpHeader, HttpResponse, Link, PreparedRequest, RequestContext, Session, Step,
};
pub use invitation::{DeviceInvitation, InvitationCheck, SetupInvitation};
pub use origin::server_origin;
