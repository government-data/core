//! The reason for a view of personal data and what the view showed
//! (decision 0069): a purpose from a fixed list, an optional checked
//! reference, and the view's shape. Never free text, never a value. The list
//! lives here so that every module audits a view with the same one.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Why values were revealed: one of a fixed list (decision 0069, point 6).
/// The keys are stable: one is never renamed, and the list only grows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ViewPurpose {
    /// Data quality review.
    QualityReview,
    /// Writing or testing rules.
    RuleWriting,
    /// Remediation of data.
    Remediation,
    /// A client's request.
    ClientRequest,
    /// Audit or internal control.
    Audit,
    /// None of the listed: requires a reference.
    Other,
}

impl ViewPurpose {
    /// Every purpose offered, in the order a person is offered them.
    pub const ALL: [Self; 6] = [
        Self::QualityReview,
        Self::RuleWriting,
        Self::Remediation,
        Self::ClientRequest,
        Self::Audit,
        Self::Other,
    ];

    /// The stable key, as stored and as the API speaks it.
    pub fn key(self) -> &'static str {
        match self {
            Self::QualityReview => "quality_review",
            Self::RuleWriting => "rule_writing",
            Self::Remediation => "remediation",
            Self::ClientRequest => "client_request",
            Self::Audit => "audit",
            Self::Other => "other",
        }
    }

    /// The purpose of a key; `None` for a key not offered.
    pub fn parse(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.key() == key)
    }
}

/// The longest reference accepted (decision 0069, point 7).
pub const REFERENCE_MAX: usize = 32;

/// A ticket or case number (`INC-2041`, `CASO/2026/118`): at most 32 ASCII
/// characters, the first a letter or a digit, then letters, digits and
/// `-_/#.`, with at least one letter, so that neither a name as typed (with
/// its space) nor a bare document or phone number fits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference(String);

impl Reference {
    /// The reference, if `raw` has the form; `None` otherwise.
    pub fn parse(raw: &str) -> Option<Self> {
        let mut chars = raw.chars();
        let first_ok = chars.next().is_some_and(|c| c.is_ascii_alphanumeric());
        let rest_ok = chars.all(|c| c.is_ascii_alphanumeric() || "-_/#.".contains(c));
        let has_letter = raw.chars().any(|c| c.is_ascii_alphabetic());
        (first_ok && rest_ok && has_letter && raw.len() <= REFERENCE_MAX)
            .then(|| Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Which part of a reason was refused: the 422's code (`purpose` or
/// `reference`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasonError {
    /// Not a key of [`ViewPurpose`].
    Purpose,
    /// Not of a reference's form, or absent with [`ViewPurpose::Other`].
    Reference,
}

impl ReasonError {
    /// The field it names, as a refusal's code.
    pub fn code(self) -> &'static str {
        match self {
            Self::Purpose => "purpose",
            Self::Reference => "reference",
        }
    }
}

/// A view's reason: its purpose and its reference, checked together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewReason {
    purpose: ViewPurpose,
    reference: Option<Reference>,
}

impl ViewReason {
    /// The reason of a purpose's key and a reference. An empty reference is
    /// absent; `other` requires one (ruled on 2026-11-13).
    pub fn parse(purpose: &str, reference: Option<&str>) -> Result<Self, ReasonError> {
        let purpose = ViewPurpose::parse(purpose).ok_or(ReasonError::Purpose)?;
        let reference = match reference.filter(|r| !r.is_empty()) {
            Some(raw) => Some(Reference::parse(raw).ok_or(ReasonError::Reference)?),
            None if purpose == ViewPurpose::Other => return Err(ReasonError::Reference),
            None => None,
        };
        Ok(Self { purpose, reference })
    }

    pub fn purpose(&self) -> ViewPurpose {
        self.purpose
    }

    pub fn reference(&self) -> Option<&Reference> {
        self.reference.as_ref()
    }
}

/// What a view showed, besides who and on what ([`crate::AuditEntry`]): its
/// reason, its job and its shape. Never a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewDetail {
    pub reason: ViewReason,
    /// The view's job (a UUID).
    pub job: String,
    /// How many rows the answer showed.
    pub rows_shown: u32,
    /// The names of the columns or fields shown, in order, each once.
    pub fields_shown: Vec<String>,
}

/// The longest console column name recorded as it is (decision 0069,
/// point 8).
pub const COLUMN_NAME_MAX: usize = 64;

/// How a console result's column is recorded: its name when it is a plain
/// identifier (`^[A-Za-z_][A-Za-z0-9_]{0,63}$`), its 1-based position
/// (`#3`) otherwise, so a free-text alias never reaches the audit.
pub fn console_column(position: usize, name: &str) -> String {
    let mut chars = name.chars();
    let plain = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && name.len() <= COLUMN_NAME_MAX;
    if plain {
        name.to_string()
    } else {
        format!("#{position}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_purpose_is_parsed_from_its_key_and_speaks_it() {
        for purpose in ViewPurpose::ALL {
            assert_eq!(ViewPurpose::parse(purpose.key()), Some(purpose));
            let json = serde_json::to_string(&purpose).unwrap();
            assert_eq!(json, format!("\"{}\"", purpose.key()));
        }
        assert_eq!(ViewPurpose::parse("QualityReview"), None);
        assert_eq!(ViewPurpose::parse(""), None);
    }

    #[test]
    fn a_reference_is_a_ticket_number() {
        for ok in ["INC-2041", "CASO/2026/118", "T-48213", "a", "9a", "x#1.b_c"] {
            assert_eq!(Reference::parse(ok).unwrap().as_str(), ok, "{ok}");
        }
        let longest = format!("A{}", "1".repeat(REFERENCE_MAX - 1));
        assert!(Reference::parse(&longest).is_some());
        let too_long = format!("A{}", "1".repeat(REFERENCE_MAX));
        for bad in [
            "",
            "12345678",
            "INC 2041",
            "INC-2041 a",
            "Ñandú-1",
            "-INC2041",
            "#12a",
            too_long.as_str(),
            "INC-2041\n",
        ] {
            assert_eq!(Reference::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_reason_checks_its_purpose_and_its_reference_together() {
        let reason = ViewReason::parse("quality_review", None).unwrap();
        assert_eq!(reason.purpose(), ViewPurpose::QualityReview);
        assert_eq!(reason.reference(), None);
        let empty = ViewReason::parse("audit", Some("")).unwrap();
        assert_eq!(empty.reference(), None, "empty is absent");
        let other = ViewReason::parse("other", Some("INC-2041")).unwrap();
        assert_eq!(other.reference().map(Reference::as_str), Some("INC-2041"));
        assert_eq!(
            ViewReason::parse("curiosity", Some("INC-2041")),
            Err(ReasonError::Purpose)
        );
        assert_eq!(
            ViewReason::parse("audit", Some("12345678")),
            Err(ReasonError::Reference)
        );
        for none in [None, Some("")] {
            assert_eq!(
                ViewReason::parse("other", none),
                Err(ReasonError::Reference)
            );
        }
        assert_eq!(ReasonError::Reference.code(), "reference");
    }

    #[test]
    fn a_console_column_keeps_a_plain_name_and_hides_an_alias() {
        assert_eq!(console_column(1, "cliente_id"), "cliente_id");
        assert_eq!(console_column(2, "_x9"), "_x9");
        let longest = "c".repeat(COLUMN_NAME_MAX);
        assert_eq!(console_column(3, &longest), longest);
        let too_long = "c".repeat(COLUMN_NAME_MAX + 1);
        for (n, alias) in [
            "Juan Perez",
            "\"name\"",
            "it's",
            "1st",
            "año",
            "",
            "a-b",
            too_long.as_str(),
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(console_column(n + 4, alias), format!("#{}", n + 4));
        }
    }
}
