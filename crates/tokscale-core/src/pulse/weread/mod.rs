pub mod cache;
mod client;
pub mod model;

pub use client::{
    fetch_current, normalize_monthly, normalize_notes, normalize_shelf, normalize_weekly,
};
pub use model::{
    format_compare_ratio, format_read_duration, now_millis, WeReadBookRef, WeReadCategory,
    WeReadDay, WeReadFocusBook, WeReadMonthly, WeReadNotebookSummary, WeReadNotesSummary,
    WeReadShelfSummary, WeReadState, WeReadStatus, WeReadWeekly,
};
