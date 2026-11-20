//! A view of personal data in the audit trail (decision 0069): the action's
//! audit, with the view's reason and what it showed. Never a value.

use crate::NewActionAudit;

/// A view's reason and shape, as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewShown {
    /// The purpose's key (`quality_review`), checked by the caller against
    /// the purposes offered; a stored key may be one no longer offered.
    pub purpose: String,
    /// A ticket or case number, as given.
    pub reference: Option<String>,
    /// How many rows the answer showed.
    pub rows_shown: u32,
    /// The names of the columns or fields shown, in order.
    pub fields_shown: Vec<String>,
}

/// A new view to persist: who, what, on what and its job (`action.job_id`),
/// with its reason and shape.
#[derive(Debug, Clone)]
pub struct NewViewAudit {
    pub action: NewActionAudit,
    pub shown: ViewShown,
}
