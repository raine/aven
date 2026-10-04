use std::fmt;

use zeroize::Zeroize;

/// Public cryptographic context supplied by the vault owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalSharedStatePackageContext {
    pub vault_id: [u8; 32],
    pub generation_id: [u8; 32],
}

/// A generation secret supplied by a secure-store boundary.
///
/// Protocol never persists this value. Debug output is redacted and owned bytes are
/// zeroized on drop. Callers remain responsible for protected durable storage.
pub struct LocalSharedStatePackageKey([u8; 32]);

impl Clone for LocalSharedStatePackageKey {
    fn clone(&self) -> Self {
        Self(self.0)
    }
}

impl LocalSharedStatePackageKey {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Borrows key bytes only for persistence at a host protected-store boundary.
    ///
    /// Callers must not place these bytes in SQLite, settings, logs, exports, or
    /// ordinary backups.
    pub fn protected_storage_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for LocalSharedStatePackageKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LocalSharedStatePackageKey([REDACTED])")
    }
}

impl Drop for LocalSharedStatePackageKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}
