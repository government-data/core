//! Integration test example for the audit module: exercises
//! [`PgAuditRepository`] against a real, ephemeral, freshly migrated Postgres
//! via `testkit`.

use std::time::{Duration, SystemTime};

use domain::{
    AdminId, AuditEventType, AuditRepository, NewActionAudit, NewAuditEvent, NewViewAudit,
    ViewShown,
};
use infrastructure::PgAuditRepository;

fn event(event_type: AuditEventType, occurred_at: SystemTime) -> NewAuditEvent {
    NewAuditEvent {
        event_type,
        admin_id: None,
        email_attempted: Some("admin@example.com".to_string()),
        ip: "203.0.113.1".to_string(),
        user_agent: Some("curl/8.0".to_string()),
        occurred_at,
    }
}

#[tokio::test]
async fn record_then_list_recent_round_trips() {
    let db = testkit::spawn_test_db().await;
    let repo = PgAuditRepository::new(db.pool.clone());

    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    repo.record(&event(AuditEventType::LoginFailed, now))
        .await
        .unwrap();

    let recent = repo.list_recent(10).await.unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].event_type, AuditEventType::LoginFailed);
    assert_eq!(
        recent[0].email_attempted.as_deref(),
        Some("admin@example.com")
    );
    assert_eq!(recent[0].ip, "203.0.113.1");
    assert_eq!(recent[0].user_agent.as_deref(), Some("curl/8.0"));
    assert_eq!(recent[0].occurred_at, now);
    assert!(recent[0].admin_id.is_none());
}

#[tokio::test]
async fn list_recent_orders_newest_first_and_respects_the_limit() {
    let db = testkit::spawn_test_db().await;
    let repo = PgAuditRepository::new(db.pool.clone());
    let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);

    for (i, event_type) in [
        AuditEventType::LoginFailed,
        AuditEventType::LoginSucceeded,
        AuditEventType::LoggedOut,
    ]
    .into_iter()
    .enumerate()
    {
        repo.record(&event(event_type, base + Duration::from_secs(i as u64)))
            .await
            .unwrap();
    }

    let recent = repo.list_recent(2).await.unwrap();
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].event_type, AuditEventType::LoggedOut);
    assert_eq!(recent[1].event_type, AuditEventType::LoginSucceeded);
}

#[tokio::test]
async fn admin_id_is_preserved_when_known() {
    let db = testkit::spawn_test_db().await;
    let repo = PgAuditRepository::new(db.pool.clone());

    // No FK to admin_users, so any well-formed UUID is accepted — the audit
    // trail must survive even if the admin row is later deleted.
    let admin_id = AdminId::new("00000000-0000-0000-0000-000000000001");
    let mut new_event = event(
        AuditEventType::LoginSucceeded,
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000),
    );
    new_event.admin_id = Some(admin_id.clone());

    repo.record(&new_event).await.unwrap();

    let recent = repo.list_recent(1).await.unwrap();
    assert_eq!(recent[0].admin_id, Some(admin_id));
}

#[tokio::test]
async fn an_action_records_who_did_what_on_what_and_lists_with_it() {
    let db = testkit::spawn_test_db().await;
    let repo = PgAuditRepository::new(db.pool.clone());
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    let admin = "00000000-0000-0000-0000-0000000000a1";
    repo.record_action(&NewActionAudit {
        event_type: AuditEventType::SecretsChanged,
        admin_id: Some(AdminId::new(admin)),
        service_account: Some("sa-1".to_string()),
        job_id: None,
        action: "quality:ManageSources".to_string(),
        resource: "source/core".to_string(),
        ip: "203.0.113.1".to_string(),
        user_agent: None,
        occurred_at: now,
    })
    .await
    .unwrap();

    let recent = repo.list_recent(10).await.unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].event_type, AuditEventType::SecretsChanged);
    assert_eq!(
        recent[0].admin_id.as_ref().map(AdminId::as_str),
        Some(admin)
    );
    assert_eq!(recent[0].action.as_deref(), Some("quality:ManageSources"));
    assert_eq!(recent[0].resource.as_deref(), Some("source/core"));
    assert!(recent[0].email_attempted.is_none());
    assert_eq!(recent[0].service_account.as_deref(), Some("sa-1"));
    assert!(recent[0].job_id.is_none());
}

#[tokio::test]
async fn credentials_issued_to_a_job_record_the_job_and_the_machine() {
    let db = testkit::spawn_test_db().await;
    let repo = PgAuditRepository::new(db.pool.clone());
    let job = "00000000-0000-0000-0000-000000000001".to_string();
    repo.record_action(&NewActionAudit {
        event_type: AuditEventType::CredentialsIssued,
        admin_id: None,
        service_account: Some("qd-worker-1".to_string()),
        job_id: Some(job.clone()),
        action: "job:extract".to_string(),
        resource: "source/core".to_string(),
        ip: "203.0.113.9".to_string(),
        user_agent: None,
        occurred_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000),
    })
    .await
    .unwrap();
    let recent = repo.list_recent(10).await.unwrap();
    assert_eq!(recent[0].event_type, AuditEventType::CredentialsIssued);
    assert!(recent[0].admin_id.is_none());
    assert_eq!(recent[0].job_id.as_deref(), Some(job.as_str()));
    assert_eq!(recent[0].service_account.as_deref(), Some("qd-worker-1"));
}

/// Decision 0069: a view records its job, its reason and its shape, and an
/// event that is not a view reads with none.
#[tokio::test]
async fn a_view_records_its_job_reason_and_shape() {
    let db = testkit::spawn_test_db().await;
    let repo = PgAuditRepository::new(db.pool.clone());
    let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    let job = "00000000-0000-0000-0000-000000000002".to_string();
    let action = |occurred_at, job_id| NewActionAudit {
        event_type: AuditEventType::RecordsViewed,
        admin_id: Some(AdminId::new("00000000-0000-0000-0000-0000000000a1")),
        service_account: None,
        job_id,
        action: "quality:ViewPersonalData".to_string(),
        resource: "source/core/clientes/page/1".to_string(),
        ip: "203.0.113.1".to_string(),
        user_agent: Some("curl/8.0".to_string()),
        occurred_at,
    };
    repo.record_action(&action(base, None)).await.unwrap();
    let shown = ViewShown {
        purpose: "client_request".to_string(),
        reference: Some("CASO/2026/118".to_string()),
        rows_shown: 50,
        fields_shown: vec!["cliente_id".to_string(), "#2".to_string()],
    };
    repo.record_view(&NewViewAudit {
        action: action(base + Duration::from_secs(1), Some(job.clone())),
        shown: shown.clone(),
    })
    .await
    .unwrap();

    let recent = repo.list_recent(10).await.unwrap();
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].event_type, AuditEventType::RecordsViewed);
    assert_eq!(recent[0].job_id.as_deref(), Some(job.as_str()));
    assert_eq!(
        recent[0].action.as_deref(),
        Some("quality:ViewPersonalData")
    );
    assert_eq!(
        recent[0].resource.as_deref(),
        Some("source/core/clientes/page/1")
    );
    assert_eq!(recent[0].user_agent.as_deref(), Some("curl/8.0"));
    assert_eq!(recent[0].view.as_ref(), Some(&shown));
    assert!(recent[1].view.is_none(), "a view before 0069 has none");
    assert!(recent[1].job_id.is_none());
}
