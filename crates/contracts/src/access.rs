//! Who calls a module's routes for people, as the composition root resolved
//! it. A module never sees a session, a cookie or a token: `server`
//! authenticates the request and puts a [`Caller`] in its extensions, and the
//! module asks the caller what it may do and records what it did. The
//! tenant is the request's already (`tenancy::within`).

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;

use crate::view::ViewDetail;

/// What the caller may do: its IAM actor, behind the composition root.
pub trait Access: Send + Sync {
    /// Whether it may do `action` (`quality:ManageSources`) on `resource`
    /// (`source/core`, or `*`).
    fn can(&self, action: &str, resource: &str) -> bool;
    /// Whether it may do `action` on some resource: to list what it may see.
    fn can_some(&self, action: &str) -> bool;
    /// Whether it may do `action` on every resource of `kind` (`pipeline`),
    /// those to come included: allowed on all, denied on none (decision
    /// 0064). Refused unless the access says otherwise.
    fn can_every(&self, _action: &str, _kind: &str) -> bool {
        false
    }
}

/// One entry of the audit log: who, what, on what. Never a value: a secret
/// is recorded as changed, by the name of its field at most.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    /// The IAM action it was done under (`quality:ManageSources`).
    pub action: &'static str,
    /// What it was done on (`source/core`).
    pub resource: String,
    pub change: Change,
}

/// What a change did to its resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Created,
    /// Its fields, not its secrets.
    Changed,
    /// Its secrets, recorded as changed, never their value.
    SecretsChanged,
    Deleted,
    /// A run started by hand (decision 0026: _Run now_).
    Ran,
    /// A draft approved: merged into `main` (decision 0021).
    Approved,
    /// Records read with their values (decisions 0019, 0043): personal
    /// data seen, recorded as who and which resource, never the values.
    Viewed,
}

/// Where audit entries go (the installation's audit log, decision 0026).
#[async_trait]
pub trait AuditTrail: Send + Sync {
    async fn record(&self, by: &Caller, entry: &AuditEntry) -> Result<(), String>;

    /// Records a view of values with its reason and shape (decision 0069).
    /// Refused unless the trail says otherwise: a trail that cannot keep a
    /// view's detail must not let the values go.
    async fn record_view(
        &self,
        _by: &Caller,
        _entry: &AuditEntry,
        _detail: &ViewDetail,
    ) -> Result<(), String> {
        Err("this audit trail does not record views".to_string())
    }
}

/// The caller of a request: its account and where it came from, what it may
/// do, and its audit log.
#[derive(Clone)]
pub struct Caller {
    /// The account; of a service account, its owner.
    pub account: String,
    /// The service account the request comes with, if any.
    pub service_account: Option<String>,
    pub ip: String,
    pub user_agent: Option<String>,
    access: Arc<dyn Access>,
    audit: Arc<dyn AuditTrail>,
}

impl fmt::Debug for Caller {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Caller")
            .field("account", &self.account)
            .field("service_account", &self.service_account)
            .finish_non_exhaustive()
    }
}

impl Caller {
    pub fn new(
        account: String,
        service_account: Option<String>,
        ip: String,
        user_agent: Option<String>,
        access: Arc<dyn Access>,
        audit: Arc<dyn AuditTrail>,
    ) -> Self {
        Self {
            account,
            service_account,
            ip,
            user_agent,
            access,
            audit,
        }
    }

    /// Whether it may do `action` on `resource`.
    pub fn can(&self, action: &str, resource: &str) -> bool {
        self.access.can(action, resource)
    }

    /// Whether it may do `action` on some resource.
    pub fn can_some(&self, action: &str) -> bool {
        self.access.can_some(action)
    }

    /// Whether it may do `action` on every resource of `kind`.
    pub fn can_every(&self, action: &str, kind: &str) -> bool {
        self.access.can_every(action, kind)
    }

    /// Records what it did. A module records after doing it, and fails the
    /// request if the log cannot take it: what is not audited is not done
    /// silently.
    pub async fn audit(&self, entry: AuditEntry) -> Result<(), String> {
        self.audit.record(self, &entry).await
    }

    /// Records a view of values (`Change::Viewed`) with its reason, job and
    /// shape, before the values go: a module answers the values only once
    /// this is done, and refuses them if it fails.
    pub async fn audit_view(&self, entry: AuditEntry, detail: ViewDetail) -> Result<(), String> {
        self.audit.record_view(self, &entry, &detail).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct OnlyView;
    impl Access for OnlyView {
        fn can(&self, action: &str, _: &str) -> bool {
            action.starts_with("quality:View")
        }
        fn can_some(&self, action: &str) -> bool {
            self.can(action, "*")
        }
    }

    #[derive(Default)]
    struct Kept(Mutex<Vec<(String, AuditEntry)>>);
    #[async_trait]
    impl AuditTrail for Kept {
        async fn record(&self, by: &Caller, entry: &AuditEntry) -> Result<(), String> {
            self.0
                .lock()
                .unwrap()
                .push((by.account.clone(), entry.clone()));
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_caller_answers_what_it_may_do_and_records_what_it_did() {
        let kept = Arc::new(Kept::default());
        let caller = Caller::new(
            "a1".into(),
            None,
            "192.0.2.1".into(),
            None,
            Arc::new(OnlyView),
            kept.clone(),
        );
        assert!(caller.can("quality:ViewSources", "source/core"));
        assert!(!caller.can("quality:ManageSources", "source/core"));
        assert!(
            !caller.can_every("quality:ViewSources", "source"),
            "refused unless said"
        );
        let entry = AuditEntry {
            action: "quality:ManageSources",
            resource: "source/core".into(),
            change: Change::Created,
        };
        caller.audit(entry.clone()).await.unwrap();
        assert_eq!(*kept.0.lock().unwrap(), [("a1".to_string(), entry)]);
    }

    #[tokio::test]
    async fn a_trail_that_does_not_record_views_refuses_them() {
        let kept = Arc::new(Kept::default());
        let caller = Caller::new(
            "a1".into(),
            None,
            "192.0.2.1".into(),
            None,
            Arc::new(OnlyView),
            kept.clone(),
        );
        let entry = AuditEntry {
            action: "quality:ViewPersonalData",
            resource: "lake/console/j1".into(),
            change: Change::Viewed,
        };
        let detail = ViewDetail {
            reason: crate::ViewReason::parse("audit", Some("INC-2041")).unwrap(),
            job: "00000000-0000-0000-0000-000000000001".into(),
            rows_shown: 2,
            fields_shown: vec!["cliente_id".into()],
        };
        assert!(caller.audit_view(entry, detail).await.is_err());
        assert!(kept.0.lock().unwrap().is_empty(), "nothing recorded");
    }
}
