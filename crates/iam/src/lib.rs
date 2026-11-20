//! Who can do what (ADR 0016), as AWS IAM, RustFS and MinIO do it: principals
//! get policies, and a policy is a list of statements that allow or deny
//! actions on resources.
//!
//! - [`catalog`]: the actions and the kinds of resource.
//! - [`policy`]: statements, policies, their validation and the patterns.
//! - [`managed`]: the policies the application ships.
//! - [`Permissions`]: the statements of a principal, evaluated: access is
//!   denied unless something allows it, and an explicit deny always wins.
//! - [`Actor`]: who makes the request and what they can do.

pub mod catalog;
pub mod managed;
pub mod policy;

pub use policy::{glob, Effect, Policy, PolicyError, Statement};

/// The role that can do everything, without policies: the installation's
/// administrators, for bootstrapping and recovery.
pub const ADMIN_ROLE: &str = "admin";

/// The statements of a principal, from all of its policies.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Permissions {
    statements: Vec<Statement>,
}

impl Permissions {
    pub fn from_policies(policies: &[Policy]) -> Self {
        Self {
            statements: policies
                .iter()
                .flat_map(|p| p.statements.iter().cloned())
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.statements.is_empty()
    }

    fn effects(&self, action: &str, resource: &str) -> (bool, bool) {
        let mut allowed = false;
        for s in self
            .statements
            .iter()
            .filter(|s| s.covers(action, resource))
        {
            match s.effect {
                Effect::Deny => return (false, true),
                Effect::Allow => allowed = true,
            }
        }
        (allowed, false)
    }

    /// Whether `action` on `resource` is allowed.
    pub fn can(&self, action: &str, resource: &str) -> bool {
        self.effects(action, resource).0
    }

    /// Whether `action` is allowed on some resource (for the UI to show what
    /// leads to it): an allow speaks of it and no deny covers every resource.
    pub fn can_some(&self, action: &str) -> bool {
        let denied_everywhere = self.statements.iter().any(|s| {
            s.effect == Effect::Deny
                && s.actions.iter().any(|p| glob(p, action))
                && s.resources.iter().any(|r| r == "*")
        });
        !denied_everywhere
            && self
                .statements
                .iter()
                .any(|s| s.effect == Effect::Allow && s.actions.iter().any(|p| glob(p, action)))
    }

    /// Which resources of `kind` allow `action`, as patterns of their names.
    pub fn scope(&self, action: &str, kind: &str) -> ScopePart {
        let mut part = ScopePart::default();
        let prefix = format!("{kind}/");
        let all_of_kind = |r: &str| r == "*" || r == format!("{kind}/*");
        for s in &self.statements {
            if !s.actions.iter().any(|p| glob(p, action)) {
                continue;
            }
            for r in &s.resources {
                match (s.effect, all_of_kind(r)) {
                    (Effect::Deny, true) => return ScopePart::default(),
                    (Effect::Allow, true) => part.all = true,
                    (effect, false) => {
                        if let Some(name) = r.strip_prefix(&prefix) {
                            match effect {
                                Effect::Allow => part.allow.push(name.to_string()),
                                Effect::Deny => part.deny.push(name.to_string()),
                            }
                        }
                    }
                }
            }
        }
        part
    }
}

/// Which names of a kind are allowed: all, or those matching `allow`; never
/// those matching `deny`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScopePart {
    pub all: bool,
    pub allow: Vec<String>,
    pub deny: Vec<String>,
}

impl ScopePart {
    fn everything() -> Self {
        Self {
            all: true,
            ..Self::default()
        }
    }

    pub fn matches(&self, name: &str) -> bool {
        (self.all || self.allow.iter().any(|p| glob(p, name)))
            && !self.deny.iter().any(|p| glob(p, name))
    }
}

/// Which resources an actor may act on, for a list: every part must match.
/// A repository turns it into a `WHERE` clause (see [`like`]), so lists are
/// never filtered in memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    pub parts: Vec<ScopePart>,
}

impl Scope {
    pub fn matches(&self, name: &str) -> bool {
        self.parts.iter().all(|p| p.matches(name))
    }

    /// Whether nothing can match (to skip the query).
    pub fn is_nothing(&self) -> bool {
        self.parts.iter().any(|p| !p.all && p.allow.is_empty())
    }
}

/// A name pattern as a SQL `LIKE` pattern (with `\` as the escape).
pub fn like(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    for c in pattern.chars() {
        match c {
            '\\' | '%' | '_' => {
                out.push('\\');
                out.push(c);
            }
            '*' => out.push('%'),
            c => out.push(c),
        }
    }
    out
}

/// Who makes a request, and what they can do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actor {
    /// The account (`admin_users` id); of a service account, its owner.
    pub admin_id: String,
    /// The service account the request comes with, if any.
    pub service_account: Option<String>,
    /// The `admin` role: everything (still within `ceiling`).
    pub is_admin: bool,
    /// From the account's policies and its groups'.
    pub permissions: Permissions,
    /// A service account's own policies: they can only narrow its owner's.
    pub ceiling: Option<Permissions>,
}

impl Actor {
    /// An account's actor.
    pub fn account(admin_id: impl Into<String>, is_admin: bool, permissions: Permissions) -> Self {
        Self {
            admin_id: admin_id.into(),
            service_account: None,
            is_admin,
            permissions,
            ceiling: None,
        }
    }

    /// Whether it may do `action` on `resource` (`*` for the actions that act
    /// on the installation as a whole).
    pub fn can(&self, action: &str, resource: &str) -> bool {
        (self.is_admin || self.permissions.can(action, resource))
            && self
                .ceiling
                .as_ref()
                .is_none_or(|c| c.can(action, resource))
    }

    /// Whether it may do an action that acts on the installation as a whole.
    pub fn may(&self, action: &str) -> bool {
        self.can(action, "*")
    }

    /// Whether it may do `action` on some resource.
    pub fn can_some(&self, action: &str) -> bool {
        (self.is_admin || self.permissions.can_some(action))
            && self.ceiling.as_ref().is_none_or(|c| c.can_some(action))
    }

    /// The actions of the catalog it may do on some resource, for the UI.
    pub fn actions(&self) -> Vec<&'static str> {
        catalog::ACTIONS
            .iter()
            .map(|a| a.name)
            .filter(|a| self.can_some(a))
            .collect()
    }

    /// Whether it may do `action` on every resource of `kind`, those to come
    /// included: allowed on all of them (`*` or `<kind>/*`) and denied on
    /// none (decision 0064).
    pub fn can_every(&self, action: &str, kind: &str) -> bool {
        self.scope(action, kind)
            .parts
            .iter()
            .all(|p| p.all && p.deny.is_empty())
    }

    /// Which resources of `kind` it may do `action` on.
    pub fn scope(&self, action: &str, kind: &str) -> Scope {
        let mut parts = vec![if self.is_admin {
            ScopePart::everything()
        } else {
            self.permissions.scope(action, kind)
        }];
        if let Some(c) = &self.ceiling {
            parts.push(c.scope(action, kind));
        }
        Scope { parts }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn perms(statements: Vec<Statement>) -> Permissions {
        Permissions { statements }
    }

    fn deny(actions: &[&str], resources: &[&str]) -> Statement {
        Statement {
            effect: Effect::Deny,
            ..Statement::allow(actions, resources)
        }
    }

    #[test]
    fn denied_unless_allowed_and_deny_wins() {
        let p = perms(vec![
            Statement::allow(&["admin:ManageSecrets"], &["secret/*"]),
            deny(&["admin:ManageSecrets"], &["secret/PROD_*"]),
        ]);
        assert!(p.can("admin:ManageSecrets", "secret/BD_TEST"));
        assert!(!p.can("admin:ManageSecrets", "secret/PROD_CORE"));
        assert!(!p.can("admin:ViewAudit", "*"));
        assert!(p.can_some("admin:ManageSecrets"));
        assert!(!perms(vec![]).can_some("admin:ManageSecrets"));
    }

    #[test]
    fn scopes_for_lists() {
        let p = perms(vec![
            Statement::allow(&["admin:ManageSecrets"], &["secret/BD_*", "secret/API"]),
            deny(&["admin:ManageSecrets"], &["secret/BD_PROD"]),
        ]);
        let s = p.scope("admin:ManageSecrets", "secret");
        assert!(!s.all);
        assert!(s.matches("BD_TEST") && s.matches("API"));
        assert!(!s.matches("BD_PROD") && !s.matches("OTHER"));

        let all = perms(vec![Statement::allow(&["*"], &["*"])]).scope("x:Y", "secret");
        assert!(all.all && all.matches("anything"));

        let nothing = perms(vec![
            Statement::allow(&["*"], &["*"]),
            deny(&["admin:ManageSecrets"], &["*"]),
        ])
        .scope("admin:ManageSecrets", "secret");
        assert!(!nothing.matches("BD"));
    }

    #[test]
    fn a_service_account_only_narrows_its_owner() {
        let owner = perms(vec![Statement::allow(&["quality:*"], &["*"])]);
        let mut sa = Actor::account("ana", false, owner);
        sa.service_account = Some("sa1".into());
        assert!(sa.can("quality:RunPipeline", "pipeline/p"));
        sa.ceiling = Some(perms(vec![Statement::allow(
            &["quality:RunPipeline", "admin:ViewAudit"],
            &["pipeline/cartera-*", "*"],
        )]));
        assert!(sa.can("quality:RunPipeline", "pipeline/cartera-1"));
        assert!(!sa.can("quality:EditRules", "catalog/c"));
        // The ceiling grants nothing its owner lacks.
        assert!(!sa.may("admin:ViewAudit"));
        let scope = sa.scope("quality:RunPipeline", "pipeline");
        assert!(scope.matches("cartera-1") && scope.matches("cartera-2"));

        let mut admin_sa = Actor::account("root", true, Permissions::default());
        admin_sa.ceiling = Some(perms(vec![Statement::allow(&["admin:ViewAudit"], &["*"])]));
        assert!(admin_sa.may("admin:ViewAudit"));
        assert!(!admin_sa.may("admin:Configure"));
        assert_eq!(admin_sa.actions(), vec!["admin:ViewAudit"]);
    }

    #[test]
    fn every_resource_of_a_kind() {
        const PERSONAL: &str = "quality:ViewPersonalData";
        let actor = |statements| Actor::account("ana", false, perms(statements));
        let all = actor(vec![Statement::allow(&[PERSONAL], &["pipeline/*"])]);
        assert!(all.can_every(PERSONAL, "pipeline"));
        assert!(!all.can_every(PERSONAL, "source"));
        let one_denied = actor(vec![
            Statement::allow(&["quality:*"], &["*"]),
            deny(&[PERSONAL], &["pipeline/credits"]),
        ]);
        assert!(
            one_denied.can(PERSONAL, "pipeline/*"),
            "the literal name is allowed"
        );
        assert!(!one_denied.can_every(PERSONAL, "pipeline"));
        let some = actor(vec![Statement::allow(&[PERSONAL], &["pipeline/cr*"])]);
        assert!(!some.can_every(PERSONAL, "pipeline"));
        assert!(
            Actor::account("root", true, Permissions::default()).can_every(PERSONAL, "pipeline")
        );
        let mut narrowed = Actor::account("root", true, Permissions::default());
        narrowed.ceiling = Some(perms(vec![Statement::allow(&[PERSONAL], &["pipeline/a"])]));
        assert!(!narrowed.can_every(PERSONAL, "pipeline"));
    }

    #[test]
    fn like_patterns() {
        assert_eq!(like("BD_*"), "BD\\_%");
        assert_eq!(like("100%"), "100\\%");
        assert_eq!(like("*"), "%");
    }
}
