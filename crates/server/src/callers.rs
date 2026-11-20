//! The modules' routes for people (`contracts::Module::user_router`): served
//! only to an authenticated session, each request with its
//! [`contracts::Caller`], built here from the session's IAM actor, so a
//! module never sees a session. What the caller does is recorded in the
//! installation's audit log (decision 0026), under its action and resource.

use std::sync::Arc;
use std::time::SystemTime;

use auth_gateway::http::auth::ClientIp;
use auth_gateway::http::rbac::ActorResolver;
use auth_gateway::http::session::{require_session, CurrentSession};
use auth_gateway::SessionService;
use axum::extract::{Request, State};
use axum::http::header::USER_AGENT;
use axum::middleware::{from_fn_with_state, Next};
use axum::response::{IntoResponse, Response};
use axum::Router;
use contracts::{Access, AuditEntry, AuditTrail, Caller, Change, ModuleRegistry, ViewDetail};
use domain::{AdminId, AuditEventType, AuditRepository, NewActionAudit, NewViewAudit, ViewShown};
use http_kit::error::ApiError;

/// Every module's routes for people, behind the session and the caller.
pub(crate) fn user_routes(
    registry: &ModuleRegistry,
    sessions: SessionService,
    audit: Arc<dyn AuditRepository>,
) -> Router {
    registry
        .user_router()
        .layer(from_fn_with_state(audit, caller))
        .layer(from_fn_with_state(sessions, require_session))
}

async fn caller(
    State(audit): State<Arc<dyn AuditRepository>>,
    ClientIp(ip): ClientIp,
    mut request: Request,
    next: Next,
) -> Response {
    let forbidden = || {
        ApiError::forbidden("forbidden", "You do not have permission to do that.").into_response()
    };
    let (Some(session), Some(resolver)) = (
        request.extensions().get::<CurrentSession>().cloned(),
        request.extensions().get::<ActorResolver>().cloned(),
    ) else {
        return forbidden();
    };
    let actor = match resolver.0.actor(&session).await {
        Ok(actor) => actor,
        Err(error) => {
            tracing::warn!(%error, "could not build the session's actor");
            return forbidden();
        }
    };
    let user_agent = request
        .headers()
        .get(USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let caller = Caller::new(
        actor.admin_id.clone(),
        actor.service_account.clone(),
        ip,
        user_agent,
        Arc::new(ActorAccess(actor)),
        Arc::new(Trail(audit)),
    );
    request.extensions_mut().insert(caller);
    next.run(request).await
}

/// What the caller may do: its IAM actor.
struct ActorAccess(iam::Actor);

impl Access for ActorAccess {
    fn can(&self, action: &str, resource: &str) -> bool {
        self.0.can(action, resource)
    }

    fn can_some(&self, action: &str) -> bool {
        self.0.can_some(action)
    }

    fn can_every(&self, action: &str, kind: &str) -> bool {
        self.0.can_every(action, kind)
    }
}

/// The audit log, as `admin_audit_events` keeps it.
struct Trail(Arc<dyn AuditRepository>);

#[async_trait::async_trait]
impl AuditTrail for Trail {
    async fn record(&self, by: &Caller, entry: &AuditEntry) -> Result<(), String> {
        let event_type = match entry.change {
            Change::Created => AuditEventType::ResourceCreated,
            Change::Changed => AuditEventType::ResourceChanged,
            Change::SecretsChanged => AuditEventType::SecretsChanged,
            Change::Deleted => AuditEventType::ResourceDeleted,
            Change::Ran => AuditEventType::RunStarted,
            Change::Approved => AuditEventType::DraftApproved,
            Change::Viewed => AuditEventType::RecordsViewed,
        };
        self.0
            .record_action(&action(by, entry, event_type, None))
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// A view of values (decision 0069): `records_viewed` with its job,
    /// reason and shape. Only a view carries a view's detail.
    async fn record_view(
        &self,
        by: &Caller,
        entry: &AuditEntry,
        detail: &ViewDetail,
    ) -> Result<(), String> {
        if entry.change != Change::Viewed {
            return Err(format!("a view's detail on a {:?} entry", entry.change));
        }
        let event_type = AuditEventType::RecordsViewed;
        let job = Some(detail.job.clone());
        self.0
            .record_view(&NewViewAudit {
                action: action(by, entry, event_type, job),
                shown: ViewShown {
                    purpose: detail.reason.purpose().key().to_string(),
                    reference: detail.reason.reference().map(|r| r.as_str().to_string()),
                    rows_shown: detail.rows_shown,
                    fields_shown: detail.fields_shown.clone(),
                },
            })
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// The audit of what `by` did under `entry`, now.
fn action(
    by: &Caller,
    entry: &AuditEntry,
    event_type: AuditEventType,
    job_id: Option<String>,
) -> NewActionAudit {
    NewActionAudit {
        event_type,
        admin_id: Some(AdminId::new(by.account.clone())),
        service_account: by.service_account.clone(),
        job_id,
        action: entry.action.to_string(),
        resource: entry.resource.clone(),
        ip: by.ip.clone(),
        user_agent: by.user_agent.clone(),
        occurred_at: SystemTime::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use domain::{AuditEvent, AuditId, NewAuditEvent, RepositoryError};

    #[derive(Default)]
    struct Kept(Mutex<Vec<NewViewAudit>>);

    #[async_trait::async_trait]
    impl AuditRepository for Kept {
        async fn record(&self, _: &NewAuditEvent) -> Result<AuditId, RepositoryError> {
            unreachable!()
        }
        async fn record_action(&self, _: &NewActionAudit) -> Result<AuditId, RepositoryError> {
            unreachable!()
        }
        async fn record_view(&self, event: &NewViewAudit) -> Result<AuditId, RepositoryError> {
            self.0.lock().unwrap().push(event.clone());
            Ok(AuditId::new("e1"))
        }
        async fn list_recent(&self, _: u32) -> Result<Vec<AuditEvent>, RepositoryError> {
            Ok(Vec::new())
        }
    }

    struct Anything;
    impl Access for Anything {
        fn can(&self, _: &str, _: &str) -> bool {
            true
        }
        fn can_some(&self, _: &str) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn a_view_is_recorded_with_its_job_reason_and_shape() {
        let kept = Arc::new(Kept::default());
        let caller = Caller::new(
            "00000000-0000-0000-0000-0000000000a1".into(),
            None,
            "192.0.2.1".into(),
            Some("test-agent".into()),
            Arc::new(Anything),
            Arc::new(Trail(kept.clone())),
        );
        let job = "00000000-0000-0000-0000-000000000001";
        let detail = ViewDetail {
            reason: contracts::ViewReason::parse("client_request", Some("INC-2041")).unwrap(),
            job: job.into(),
            rows_shown: 3,
            fields_shown: vec!["cliente_id".into(), "#2".into()],
        };
        let mut entry = AuditEntry {
            action: "quality:ViewPersonalData",
            resource: "lake/console/1".into(),
            change: Change::Changed,
        };
        assert!(caller
            .audit_view(entry.clone(), detail.clone())
            .await
            .is_err());
        entry.change = Change::Viewed;
        caller.audit_view(entry, detail).await.unwrap();
        let kept = kept.0.lock().unwrap();
        let [view] = kept.as_slice() else {
            panic!("one row, not {}", kept.len())
        };
        assert_eq!(view.action.event_type, AuditEventType::RecordsViewed);
        assert_eq!(view.action.job_id.as_deref(), Some(job));
        assert_eq!(view.action.ip, "192.0.2.1");
        assert_eq!(view.action.user_agent.as_deref(), Some("test-agent"));
        assert_eq!(view.shown.purpose, "client_request");
        assert_eq!(view.shown.reference.as_deref(), Some("INC-2041"));
        assert_eq!(
            (view.shown.rows_shown, view.shown.fields_shown.len()),
            (3, 2)
        );
    }
}
