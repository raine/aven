//! Setup and device invitations, encoded as described in
//! `crate::sync::invitation_text`. Debug output is redacted.
use std::fmt;

use anyhow::Result;
use zeroize::Zeroizing;

use crate::sync::invitation_text::{self, Kind};
use crate::sync::seed_claim::{Secret, membership::Invitation};

/// Authority to claim an unclaimed server's storage and start a sync.
pub struct SetupInvitation {
    pub server: String,
    pub setup_id: [u8; 32],
    pub secret: Secret,
}

/// Authority to join the sync whose device issued it.
pub struct DeviceInvitation {
    pub server: String,
    pub invitation: Invitation,
}

impl fmt::Debug for SetupInvitation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SetupInvitation([REDACTED])")
    }
}

impl fmt::Debug for DeviceInvitation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DeviceInvitation([REDACTED])")
    }
}

impl SetupInvitation {
    pub fn encode(&self) -> Result<Zeroizing<String>> {
        let mut secret = Zeroizing::new(self.setup_id.to_vec());
        secret.extend_from_slice(self.secret.expose());
        invitation_text::encode(Kind::Setup, &self.server, &secret)
    }

    pub fn decode(text: &str) -> Result<Self> {
        Self::decode_fields(text)
            .ok_or_else(|| anyhow::anyhow!("error sync-setup-invitation-invalid"))
    }

    fn decode_fields(text: &str) -> Option<Self> {
        let decoded = invitation_text::decode(text).filter(|d| d.kind == Kind::Setup)?;
        Some(Self {
            setup_id: decoded.secret[..32].try_into().ok()?,
            secret: Secret::new(decoded.secret[32..].try_into().ok()?),
            server: decoded.server,
        })
    }
}

impl DeviceInvitation {
    pub fn encode(&self) -> Result<Zeroizing<String>> {
        // Protected storage order is vault, inviter HPKE public key, PSK,
        // matching the invitation text.
        let fields = self.invitation.protected_storage_bytes();
        invitation_text::encode(Kind::Device, &self.server, &fields)
    }

    pub fn decode(text: &str) -> Result<Self> {
        Self::decode_fields(text)
            .ok_or_else(|| anyhow::anyhow!("error sync-device-invitation-invalid"))
    }

    fn decode_fields(text: &str) -> Option<Self> {
        let decoded = invitation_text::decode(text).filter(|d| d.kind == Kind::Device)?;
        Some(Self {
            invitation: Invitation::from_protected_storage(&decoded.secret).ok()?,
            server: decoded.server,
        })
    }
}

/// What pasted text looks like as an invitation. Holds only the server
/// origin, never the secret.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum InvitationCheck {
    #[default]
    Empty,
    Setup(String),
    Device(String),
    /// Has an invitation prefix but doesn't decode.
    Incomplete,
    Unknown,
}

impl InvitationCheck {
    pub fn of(text: &str) -> Self {
        if text.trim().is_empty() {
            return Self::Empty;
        }
        match invitation_text::decode(text) {
            Some(decoded) if decoded.kind == Kind::Setup => Self::Setup(decoded.server),
            Some(decoded) => Self::Device(decoded.server),
            None if Kind::of_prefix(text).is_some() => Self::Incomplete,
            None => Self::Unknown,
        }
    }
}

/// Encoded setup and device invitations for `server`, for tests outside
/// this module.
#[cfg(any(test, feature = "test-support"))]
pub fn sample_invitations(server: &str) -> (Zeroizing<String>, Zeroizing<String>) {
    let setup = SetupInvitation {
        server: server.into(),
        setup_id: [3; 32],
        secret: Secret::new([4; 32]),
    };
    let mut storage = vec![5; 32];
    storage.extend([6; 32]);
    storage.extend([7; 32]);
    let device = DeviceInvitation {
        server: server.into(),
        invitation: Invitation::from_protected_storage(&storage).expect("sample invitation"),
    };
    (
        setup.encode().expect("sample invitation"),
        device.encode().expect("sample invitation"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invitations_round_trip_and_report_their_kind() {
        let server = "https://sync.example.net";
        let (setup_text, device_text) = sample_invitations(server);

        assert!(setup_text.starts_with(invitation_text::SETUP_PREFIX));
        let decoded = SetupInvitation::decode(&format!("  {}\n", setup_text.as_str())).unwrap();
        assert_eq!(decoded.server, server);
        assert_eq!(decoded.setup_id, [3; 32]);
        assert_eq!(decoded.secret.expose(), &[4; 32]);
        assert_eq!(format!("{decoded:?}"), "SetupInvitation([REDACTED])");
        assert!(DeviceInvitation::decode(&setup_text).is_err());

        assert!(device_text.starts_with(invitation_text::DEVICE_PREFIX));
        let decoded = DeviceInvitation::decode(&device_text.to_ascii_lowercase()).unwrap();
        assert_eq!(decoded.server, server);
        assert_eq!(decoded.encode().unwrap(), device_text);
        assert!(SetupInvitation::decode(&device_text).is_err());

        assert_eq!(
            InvitationCheck::of(&setup_text),
            InvitationCheck::Setup(server.into())
        );
        assert_eq!(
            InvitationCheck::of(&device_text),
            InvitationCheck::Device(server.into())
        );
        assert_eq!(
            InvitationCheck::of(&setup_text[..60]),
            InvitationCheck::Incomplete
        );
        assert_eq!(
            InvitationCheck::of(&device_text[..40].to_ascii_lowercase()),
            InvitationCheck::Incomplete
        );
        assert_eq!(InvitationCheck::of("hello"), InvitationCheck::Unknown);
        assert_eq!(InvitationCheck::of("  "), InvitationCheck::Empty);
    }
}
