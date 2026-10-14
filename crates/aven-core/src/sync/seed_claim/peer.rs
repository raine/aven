//! Shared invitation and installation key primitives with SQLite persistence.
pub use aven_protocol::claim::peer::*;
pub(crate) mod persistence;
use super::*;
fn check(condition: bool) -> Result<()> {
    ensure!(condition, "error enrollment-invalid");
    Ok(())
}
