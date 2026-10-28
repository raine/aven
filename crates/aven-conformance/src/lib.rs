//! Development-only behavioral scenarios shared by server backends.
//!
//! Scenarios depend only on protocol types; callers own fixture construction and
//! backend-specific storage and fault controls. These checks complement backend
//! crash and rollback tests, rather than providing a production storage interface.
pub mod bootstrap;
pub mod capability;
pub mod claim;
pub mod driver;
pub mod enrollment;
pub mod refusal;
pub mod tail;
