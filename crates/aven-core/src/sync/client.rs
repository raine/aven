//! The end-to-end encrypted sync client shared by every host.
pub mod invitation;
pub mod keys;
mod origin;

pub use invitation::{DeviceInvitation, InvitationCheck, SetupInvitation};
pub use origin::server_origin;
