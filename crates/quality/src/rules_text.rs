//! A lint finding said in a language (decision 0019, point 8): the text of
//! its code with the values it names, and the suggestion for a typo. The
//! command line and the rule editor's routes say it the same way.

use quality_rules::lint::Finding;
use texts::{text_in, Language};

/// `f`'s message in `language`.
pub fn message(language: Language, f: &Finding) -> String {
    let values: Vec<(&str, String)> = f
        .values
        .iter()
        .map(|(name, value)| match name.strip_suffix("_key") {
            Some(plain) => (plain, text_in(language, value, &[])),
            None => (*name, value.clone()),
        })
        .collect();
    let message = text_in(language, &f.code.key(), &values)
        .trim_end()
        .to_string();
    match f.values.iter().find(|(n, _)| *n == "suggestion") {
        Some((_, s)) => format!(
            "{message}. {}",
            text_in(
                language,
                "quality.lint.suggestion",
                &[("suggestion", s.clone())]
            )
        ),
        None => message,
    }
}
