//! Storage-independent HTTP payloads. Transport and admission remain backend-owned.
pub mod bootstrap;
pub mod enrollment;
pub mod images;
pub mod tail;

#[cfg(test)]
mod tests;
