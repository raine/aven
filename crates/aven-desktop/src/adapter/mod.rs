//! The bridge between GPUI views and `aven-host`, with no GPUI types.
//!
//! One thread owns the `TuiStore` and runs requests in order, so its replies keep
//! their order too. Outside writes surface through SQLite's `data_version`, which
//! changes when another connection commits. A background task runs sync rounds.

mod actor;
mod model;
mod prefs;
mod rows;

pub use actor::{create_secret_gist, first_project_name, first_project_prefix, open_store, start};
pub(crate) use actor::{failed, prepare};
pub use model::*;
pub use prefs::*;
pub(crate) use rows::capitalised;
pub use rows::{
    arranged_snapshot, available_text, detail, done_group, due_text, relative_time, short_date,
    snapshot,
};
