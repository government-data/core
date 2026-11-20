//! The texts the server shows to people, in Spanish and English.
//!
//! The catalog lives in `locales/es.json` and `locales/en.json` (nested keys,
//! `{{name}}` for the values and `_one`/`_other` for the plural with
//! `count`), compiled into the binary. Each text is asked for with [`tr!`] in
//! the current task's language: the API sets it per request with [`within`]
//! (from the `Accept-Language` the UI sends) and the background tasks with
//! each company's language. With no language set, Spanish.

use std::collections::HashMap;
use std::future::Future;
use std::sync::LazyLock;

use serde_json::Value;

/// The server's languages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Language {
    #[default]
    Es,
    En,
}

impl Language {
    pub fn code(self) -> &'static str {
        match self {
            Self::Es => "es",
            Self::En => "en",
        }
    }

    /// `es` / `en` (or `en-US`…); anything else is not a server language.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().split(['-', '_']).next()? {
            "es" => Some(Self::Es),
            "en" => Some(Self::En),
            _ => None,
        }
    }

    /// The first one the server has from an `Accept-Language`
    /// (`en-US,en;q=0.9,es;q=0.8`), in the order they come; if none,
    /// Spanish.
    pub fn from_accept_language(header: &str) -> Self {
        header
            .split(',')
            .filter_map(|part| Self::parse(part.split(';').next().unwrap_or("")))
            .next()
            .unwrap_or_default()
    }
}

tokio::task_local! {
    static CURRENT: Language;
}

/// Runs `f` in a language.
pub async fn within<F: Future>(language: Language, f: F) -> F::Output {
    CURRENT.scope(language, f).await
}

/// The current task's language (Spanish if none was set).
pub fn current() -> Language {
    CURRENT.try_with(|l| *l).unwrap_or_default()
}

fn flatten(prefix: &str, value: &Value, out: &mut HashMap<String, String>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten(&key, v, out);
            }
        }
        Value::String(s) => {
            out.insert(prefix.to_string(), s.clone());
        }
        _ => {}
    }
}

fn load(json: &str) -> HashMap<String, String> {
    let value: Value = serde_json::from_str(json).expect("a valid text catalog");
    let mut map = HashMap::new();
    flatten("", &value, &mut map);
    map
}

static ES: LazyLock<HashMap<String, String>> =
    LazyLock::new(|| load(include_str!("../locales/es.json")));
static EN: LazyLock<HashMap<String, String>> =
    LazyLock::new(|| load(include_str!("../locales/en.json")));

fn catalog(language: Language) -> &'static HashMap<String, String> {
    match language {
        Language::Es => &ES,
        Language::En => &EN,
    }
}

/// A catalog's keys (for the tests).
pub fn keys(language: Language) -> Vec<String> {
    let mut v: Vec<String> = catalog(language).keys().cloned().collect();
    v.sort();
    v
}

fn find(language: Language, key: &str, count: Option<i64>) -> Option<&'static String> {
    let cat = catalog(language);
    if let Some(n) = count {
        let form = if n == 1 { "one" } else { "other" };
        if let Some(t) = cat.get(&format!("{key}_{form}")) {
            return Some(t);
        }
    }
    cat.get(key)
}

/// The text of `key` in the current language with its values. If it is
/// missing in English the Spanish one is used; if it is missing in both, the
/// key (the tests check that it does not happen).
pub fn text(key: &str, values: &[(&str, String)]) -> String {
    text_in(current(), key, values)
}

/// The text of `key` in a given language, for code that holds the language
/// itself instead of running within one (the rules' explanations).
pub fn text_in(language: Language, key: &str, values: &[(&str, String)]) -> String {
    let count = values
        .iter()
        .find(|(n, _)| *n == "count")
        .and_then(|(_, v)| v.parse::<i64>().ok());
    let template = find(language, key, count)
        .or_else(|| find(Language::Es, key, count))
        .map(String::as_str)
        .unwrap_or(key);
    let mut out = template.to_string();
    for (name, value) in values {
        out = out.replace(&format!("{{{{{name}}}}}"), value);
    }
    out
}

/// A catalog text in the current language: `tr!("app.platform.notFound")`,
/// `tr!("app.account.greetingName", name = user.name)`.
#[macro_export]
macro_rules! tr {
    ($key:literal) => {
        $crate::text($key, &[])
    };
    ($key:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::text($key, &[$((stringify!($name), ($value).to_string())),+])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_accept_language() {
        assert_eq!(
            Language::from_accept_language("en-US,en;q=0.9,es;q=0.8"),
            Language::En
        );
        assert_eq!(Language::from_accept_language("es-CO"), Language::Es);
        assert_eq!(Language::from_accept_language("fr-FR,de"), Language::Es);
        assert_eq!(Language::from_accept_language(""), Language::Es);
    }

    /// Every `tr!("key")` in the server's code has its text.
    #[test]
    fn every_key_used_in_the_code_exists() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut pending = vec![root];
        let mut missing = Vec::new();
        let keys = keys(Language::Es);
        while let Some(dir) = pending.pop() {
            for e in std::fs::read_dir(&dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    // This crate itself: its examples are not keys.
                    if !p.ends_with("target") && !p.ends_with("texts") {
                        pending.push(p);
                    }
                } else if p.extension().is_some_and(|x| x == "rs") {
                    let source = std::fs::read_to_string(&p).unwrap();
                    for part in source.split("texts::tr!(").skip(1) {
                        let Some(rest) = part.trim_start().strip_prefix('"') else {
                            continue;
                        };
                        let key = rest.split('"').next().unwrap_or_default();
                        let plural = [format!("{key}_one"), format!("{key}_other")];
                        let exists = keys.iter().any(|c| c == key || plural.contains(c));
                        if !exists && key != "no.such.key" && !key.contains('{') {
                            missing.push(format!("{}: {key}", p.display()));
                        }
                    }
                }
            }
        }
        assert!(missing.is_empty(), "keys without a text: {missing:#?}");
    }

    #[test]
    fn both_catalogs_have_the_same_keys() {
        assert_eq!(keys(Language::Es), keys(Language::En));
    }

    #[tokio::test]
    async fn translates_in_the_current_language() {
        assert_eq!(tr!("test.hello", name = "Ana"), "Hola, Ana");
        let en = within(Language::En, async {
            (
                tr!("test.hello", name = "Ana"),
                tr!("test.workflows", count = 3),
            )
        })
        .await;
        assert_eq!(en, ("Hi, Ana".to_string(), "3 workflows".to_string()));
        assert_eq!(tr!("test.workflows", count = 1), "1 flujo");
        assert_eq!(tr!("no.such.key"), "no.such.key");
    }
}
