//! A module's route and menu entry, as the update agent tells them with the
//! installed modules (quality-sdk decision 0002, the amendment to steps 3,
//! 7 and 9): the path its route serves on the web entry point, and the
//! entry the apps menu shows, in English and Spanish.

use serde::{Deserialize, Serialize};

/// The longest label taken.
const MAX_LABEL: usize = 80;
/// The longest path taken.
const MAX_PATH: usize = 200;

/// One text in each language the web speaks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InLanguages {
    pub en: String,
    pub es: String,
}

impl InLanguages {
    /// The text in `language` (`es`, else English).
    pub fn get(&self, language: &str) -> &str {
        if language == "es" {
            &self.es
        } else {
            &self.en
        }
    }
}

/// The entry of a module in the apps menu: its name and the page it opens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleMenu {
    pub label: InLanguages,
    pub path: InLanguages,
}

/// Why a module's route or menu is refused (the message's key).
pub(super) enum Refusal {
    BadRoutePath,
    MenuWithoutRoute,
    BadMenu,
}

/// Check a module's `route_path` and `menu`: the route is one segment
/// (`/docs`), the menu needs the route, its labels are printable and its
/// paths stay under the route.
pub(super) fn check(route_path: Option<&str>, menu: Option<&ModuleMenu>) -> Result<(), Refusal> {
    if let Some(route) = route_path {
        if !is_route_path(route) {
            return Err(Refusal::BadRoutePath);
        }
    }
    let Some(menu) = menu else {
        return Ok(());
    };
    let Some(route) = route_path else {
        return Err(Refusal::MenuWithoutRoute);
    };
    let labels_ok = [&menu.label.en, &menu.label.es].iter().all(|l| {
        let l = l.trim();
        !l.is_empty() && l.chars().count() <= MAX_LABEL && !l.chars().any(char::is_control)
    });
    let paths_ok = [&menu.path.en, &menu.path.es]
        .iter()
        .all(|p| is_under(p, route));
    if labels_ok && paths_ok {
        Ok(())
    } else {
        Err(Refusal::BadMenu)
    }
}

/// `/` and one segment of lower-case letters, digits and `-`.
fn is_route_path(path: &str) -> bool {
    path.strip_prefix('/').is_some_and(|seg| {
        !seg.is_empty()
            && path.len() <= MAX_PATH
            && seg
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    })
}

/// The route itself or a path under it, with no query, fragment, `..` or
/// empty segment.
fn is_under(path: &str, route: &str) -> bool {
    let rest = match path.strip_prefix(route) {
        Some("") => return true,
        Some(rest) => rest,
        None => return false,
    };
    let Some(rest) = rest.strip_prefix('/') else {
        return false;
    };
    // Only the last segment may be empty (a trailing `/`).
    let segments: Vec<&str> = rest.split('/').collect();
    let last = segments.len() - 1;
    path.len() <= MAX_PATH
        && segments
            .iter()
            .enumerate()
            .all(|(i, s)| (i == last || !s.is_empty()) && *s != "." && *s != "..")
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu(en: &str, es: &str) -> ModuleMenu {
        ModuleMenu {
            label: InLanguages {
                en: "User guide".into(),
                es: "Guía de usuario".into(),
            },
            path: InLanguages {
                en: en.into(),
                es: es.into(),
            },
        }
    }

    #[test]
    fn a_route_and_its_menu_are_taken() {
        assert!(check(Some("/docs"), Some(&menu("/docs/", "/docs/es/"))).is_ok());
        assert!(check(Some("/docs"), Some(&menu("/docs", "/docs/es/inicio"))).is_ok());
        assert!(check(Some("/docs"), None).is_ok());
        assert!(check(None, None).is_ok());
    }

    #[test]
    fn a_bad_route_is_refused() {
        for bad in ["docs", "/", "/Docs", "/docs/", "/a/b", "/docs?x", "/do cs"] {
            assert!(
                matches!(check(Some(bad), None), Err(Refusal::BadRoutePath)),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_menu_needs_the_route_and_stays_under_it() {
        assert!(matches!(
            check(None, Some(&menu("/docs/", "/docs/es/"))),
            Err(Refusal::MenuWithoutRoute)
        ));
        for (en, es) in [
            ("/docsx/", "/docs/"),
            ("/other/", "/docs/"),
            ("/docs/../api/", "/docs/"),
            ("/docs//x", "/docs/"),
            ("/docs/?q=1", "/docs/"),
            ("https://evil.test/docs/", "/docs/"),
        ] {
            assert!(
                matches!(
                    check(Some("/docs"), Some(&menu(en, es))),
                    Err(Refusal::BadMenu)
                ),
                "{en}"
            );
        }
        let mut no_label = menu("/docs/", "/docs/");
        no_label.label.es = " ".into();
        assert!(matches!(
            check(Some("/docs"), Some(&no_label)),
            Err(Refusal::BadMenu)
        ));
    }

    #[test]
    fn the_text_in_the_language_asked() {
        let m = menu("/docs/", "/docs/es/");
        assert_eq!(m.path.get("es"), "/docs/es/");
        assert_eq!(m.path.get("en"), "/docs/");
        assert_eq!(m.path.get("fr"), "/docs/");
    }
}
