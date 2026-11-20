//! Policies: lists of statements that allow or deny actions on resources.
//!
//! Actions and resources are patterns where `*` stands for any run of
//! characters: `quality:View*`, `secret/BD_*`, `*`.

use serde::{Deserialize, Serialize};

use crate::catalog::{ACTIONS, RESOURCE_KINDS};

/// Whether a statement allows or denies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Effect {
    Allow,
    Deny,
}

/// One statement of a policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Statement {
    pub effect: Effect,
    pub actions: Vec<String>,
    pub resources: Vec<String>,
}

impl Statement {
    pub fn allow(actions: &[&str], resources: &[&str]) -> Self {
        Self {
            effect: Effect::Allow,
            actions: actions.iter().map(|a| a.to_string()).collect(),
            resources: resources.iter().map(|r| r.to_string()).collect(),
        }
    }

    /// Whether it speaks of `action` on `resource`.
    pub fn covers(&self, action: &str, resource: &str) -> bool {
        self.actions.iter().any(|p| glob(p, action))
            && self.resources.iter().any(|p| glob(p, resource))
    }
}

/// A policy: a name, what it is for, and its statements.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Policy {
    pub name: String,
    pub description: String,
    pub statements: Vec<Statement>,
    /// Shipped with the application: it can be attached, not changed.
    pub managed: bool,
}

/// Why a policy is not valid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyError {
    /// The name: lowercase letters, digits and `-`, up to 64, starting with a
    /// letter.
    Name,
    /// The name of a managed policy.
    Reserved,
    /// A policy without statements, or a statement without actions or
    /// resources.
    Empty,
    /// An action pattern that matches no action of the catalog.
    UnknownAction(String),
    /// A resource that is not `*` nor `<kind>/<pattern>` of a known kind.
    BadResource(String),
    /// A resource of a kind none of the statement's actions acts on, or an
    /// action that acts on none of its resources' kinds (decision 0064).
    KindMismatch(String),
}

impl std::fmt::Display for PolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Name => f.write_str(
                "name: lowercase letters, digits and -, up to 64, starting with a letter",
            ),
            Self::Reserved => f.write_str("name: it is a managed policy"),
            Self::Empty => f.write_str("statements: each one needs actions and resources"),
            Self::UnknownAction(a) => write!(f, "actions: {a} matches no action"),
            Self::BadResource(r) => write!(f, "resources: {r} is not * nor <kind>/<name>"),
            Self::KindMismatch(x) => {
                write!(
                    f,
                    "statements: {x} does not match the kind of the other side"
                )
            }
        }
    }
}

impl std::error::Error for PolicyError {}

/// Whether `name` can name a policy.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 64
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Check a custom policy's name and statements.
pub fn validate(name: &str, statements: &[Statement]) -> Result<(), PolicyError> {
    if !valid_name(name) {
        return Err(PolicyError::Name);
    }
    if crate::managed::find(name).is_some() {
        return Err(PolicyError::Reserved);
    }
    if statements.is_empty() {
        return Err(PolicyError::Empty);
    }
    for s in statements {
        if s.actions.is_empty() || s.resources.is_empty() {
            return Err(PolicyError::Empty);
        }
        for a in &s.actions {
            if !ACTIONS.iter().any(|known| glob(a, known.name)) {
                return Err(PolicyError::UnknownAction(a.clone()));
            }
        }
        for r in &s.resources {
            let ok = r == "*"
                || r.split_once('/')
                    .is_some_and(|(kind, rest)| RESOURCE_KINDS.contains(&kind) && !rest.is_empty());
            if !ok {
                return Err(PolicyError::BadResource(r.clone()));
            }
        }
        kinds_match(s)?;
    }
    Ok(())
}

/// Whether a statement's actions and resources are of the same kinds, as
/// the catalog has them (decision 0064): each resource of a kind some action
/// acts on, each action acting on some resource's kind. `*` is of every
/// kind, and a global action's resource is `*`.
fn kinds_match(s: &Statement) -> Result<(), PolicyError> {
    fn kind(r: &str) -> Option<&str> {
        r.split_once('/').map(|(k, _)| k)
    }
    let acts_on = |pattern: &str, kind: Option<&str>| {
        ACTIONS
            .iter()
            .any(|a| glob(pattern, a.name) && kind.is_none_or(|k| a.resource == Some(k)))
    };
    if let Some(r) = s
        .resources
        .iter()
        .find(|r| !s.actions.iter().any(|a| acts_on(a, kind(r))))
    {
        return Err(PolicyError::KindMismatch(r.clone()));
    }
    match s
        .actions
        .iter()
        .find(|a| !s.resources.iter().any(|r| acts_on(a, kind(r))))
    {
        Some(a) => Err(PolicyError::KindMismatch(a.clone())),
        None => Ok(()),
    }
}

/// Whether `text` matches `pattern`, where `*` stands for any run of
/// characters (none included). Case-sensitive.
pub fn glob(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    // Greedy with backtracking to the last `*`: linear in practice.
    let (mut i, mut j) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while j < t.len() {
        if i < p.len() && p[i] == '*' {
            star = Some((i, j));
            i += 1;
        } else if i < p.len() && p[i] == t[j] {
            i += 1;
            j += 1;
        } else if let Some((si, sj)) = star {
            i = si + 1;
            j = sj + 1;
            star = Some((si, sj + 1));
        } else {
            return false;
        }
    }
    p[i..].iter().all(|c| *c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob("*", ""));
        assert!(glob("*", "quality:ViewRules"));
        assert!(glob("quality:View*", "quality:ViewRules"));
        assert!(!glob("quality:View*", "quality:EditRules"));
        assert!(glob("secret/BD_*", "secret/BD_INFORMES"));
        assert!(!glob("secret/BD_*", "secret/API_KEY"));
        assert!(glob("dataset/*.clientes", "dataset/finanzas.clientes"));
        assert!(glob("a*b*c", "aXXbYYc"));
        assert!(!glob("a*b*c", "aXXbYY"));
        assert!(glob("exact", "exact"));
        assert!(!glob("exact", "exactly"));
    }

    #[test]
    fn validation() {
        let ok = vec![Statement::allow(
            &["quality:View*"],
            &["pipeline/cartera-*"],
        )];
        assert_eq!(validate("cartera-lectura", &ok), Ok(()));
        assert_eq!(validate("Cartera", &ok), Err(PolicyError::Name));
        assert_eq!(validate("quality-viewer", &ok), Err(PolicyError::Reserved));
        assert_eq!(validate("x", &[]), Err(PolicyError::Empty));
        let bad_action = vec![Statement::allow(&["quality:Fly"], &["*"])];
        assert!(matches!(
            validate("x", &bad_action),
            Err(PolicyError::UnknownAction(_))
        ));
        let bad_resource = vec![Statement::allow(&["*"], &["planet/mars"])];
        assert!(matches!(
            validate("x", &bad_resource),
            Err(PolicyError::BadResource(_))
        ));
    }

    #[test]
    fn actions_and_resources_of_the_same_kind() {
        let on = |actions: &[&str], resources: &[&str]| {
            validate("x", &[Statement::allow(actions, resources)])
        };
        let mismatch = |x: &str| Err(PolicyError::KindMismatch(x.into()));
        assert_eq!(
            on(&["quality:ViewPersonalData"], &["source/core"]),
            mismatch("source/core")
        );
        assert_eq!(
            on(&["admin:Configure"], &["secret/a"]),
            mismatch("secret/a")
        );
        assert_eq!(
            on(
                &["quality:ViewSources", "admin:Configure"],
                &["source/core"]
            ),
            mismatch("admin:Configure")
        );
        assert_eq!(
            on(&["quality:ViewPersonalData"], &["pipeline/credits"]),
            Ok(())
        );
        assert_eq!(on(&["quality:ViewPersonalData"], &["*"]), Ok(()));
        assert_eq!(on(&["*"], &["source/core"]), Ok(()));
        assert_eq!(on(&["quality:View*"], &["source/*", "pipeline/*"]), Ok(()));
        assert_eq!(on(&["admin:Configure"], &["*"]), Ok(()));
        // What the managed policies hold stays valid.
        for p in crate::managed::all() {
            for s in &p.statements {
                assert_eq!(kinds_match(s), Ok(()), "{}", p.name);
            }
        }
    }

    #[test]
    fn statements_read_and_write_as_json() {
        let s: Statement = serde_json::from_str(
            r#"{"effect":"Deny","actions":["quality:ViewPersonalData"],"resources":["*"]}"#,
        )
        .unwrap();
        assert_eq!(s.effect, Effect::Deny);
        assert!(s.covers("quality:ViewPersonalData", "pipeline/x"));
    }
}
