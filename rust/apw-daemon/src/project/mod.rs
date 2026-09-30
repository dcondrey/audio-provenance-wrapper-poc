//! Saved-project parsing: the Rust counterpart of `daemon/project_formats/` and the
//! `.als` parser and differ in `daemon/project_differ/`. Every parser returns the
//! same [`ProjectSnapshot`]; `tests/project_parity.rs` compares each to the Python
//! oracle's golden JSON.

pub mod als;
pub mod ardour;
pub mod dawproject;
pub mod diff;
pub mod lmms;
pub mod maxpat;
pub mod num;
pub mod puredata;
pub mod reaper;
pub mod registry;
pub mod safe;
pub mod snapshot;
pub mod tarzst;
pub mod tracker;
pub mod vcv;
pub mod xml;

pub use diff::{compute_diff, diff_to_event, session_facts, ProjectDiff};
pub use registry::{
    detect_format, parse_project, unsupported_format_event, unsupported_message, ProjectFormat, Status, FORMATS,
};
pub use safe::Limits;
pub use snapshot::{snapshot_to_golden, ProjectSnapshot};
