use super::testing::*;
use super::*;
use std::sync::Mutex;
use std::time::SystemTime;

use crate::{AdminAccount, AuthorizeParams, LockoutState, PasswordHash};

// A provider whose authorize URL is trivial and whose exchange returns a
// fixed identity — enough to drive the flow without real HTTP.
struct FakeProvider {
    id: ProviderId,
    identity: OAuthIdentity,
    last_request: Mutex<Option<ExchangeRequest>>,
}

#[async_trait::async_trait]
impl OAuthProvider for FakeProvider {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }
    fn authorize_url(&self, params: &AuthorizeParams) -> String {
        format!(
            "https://provider.test/authorize?state={}&challenge={}&nonce={}",
            params.state, params.code_challenge, params.nonce
        )
    }
    async fn exchange_code(&self, request: &ExchangeRequest) -> Result<OAuthIdentity, OAuthError> {
        *self.last_request.lock().unwrap() = Some(request.clone());
        Ok(self.identity.clone())
    }
}

fn seed_admin(admins: &InMemoryAdmins, email: &str, role: Role) -> AdminId {
    let id = AdminId::new(format!("id-{email}"));
    admins.accounts.lock().unwrap().push(AdminAccount {
        id: id.clone(),
        email: Email::parse(email).unwrap(),
        password_hash: PasswordHash::from_encoded("x"),
        lockout: LockoutState::clear(),
        role,
        display_name: None,
    });
    id
}

fn service(
    admins: Arc<InMemoryAdmins>,
    identities: Arc<InMemoryIdentities>,
    identity: OAuthIdentity,
) -> (OAuthLoginService, Arc<InMemoryPending>) {
    let pending = Arc::new(InMemoryPending::default());
    let provider = Arc::new(FakeProvider {
        id: ProviderId::parse("google").unwrap(),
        identity,
        last_request: Mutex::new(None),
    });
    let svc = OAuthLoginService::new(
        vec![provider],
        pending.clone(),
        identities,
        admins,
        Arc::new(FixedSecrets),
        Arc::new(FixedClock(SystemTime::UNIX_EPOCH)),
        "https://admin.example.com",
    );
    (svc, pending)
}

/// An identity whose email the provider verified.
fn identity(email: &str) -> OAuthIdentity {
    OAuthIdentity {
        provider: ProviderId::parse("google").unwrap(),
        subject: "sub-1".to_string(),
        email: Some(email.to_string()),
        email_verified: true,
        picture: None,
    }
}

/// Sign in through the whole flow, returning the admin it resolved to.
async fn sign_in(svc: &OAuthLoginService) -> Result<OAuthAuthenticatedAdmin, OAuthError> {
    svc.begin("google").await.unwrap();
    match svc.complete("state-123", "the-code", None).await? {
        CompleteOutcome::SignedIn(a) => Ok(a),
        other => panic!("expected a sign-in, got {other:?}"),
    }
}

#[tokio::test]
async fn begin_persists_pending_and_builds_url() {
    let (svc, pending) = service(
        Arc::new(InMemoryAdmins::default()),
        Arc::new(InMemoryIdentities::default()),
        identity("admin@example.com"),
    );
    let outcome = svc.begin("google").await.unwrap();
    assert!(outcome.authorize_url.contains("state=state-123"));
    assert!(outcome.authorize_url.contains("challenge=challenge-xyz"));
    // The pending row exists, keyed by state, with the callback URL.
    let stored = pending
        .rows
        .lock()
        .unwrap()
        .get("state-123")
        .cloned()
        .unwrap();
    assert_eq!(
        stored.redirect_uri,
        "https://admin.example.com/auth/oauth/google/callback"
    );
    assert_eq!(stored.code_verifier, "verifier-xyz");
}

#[tokio::test]
async fn begin_rejects_an_unknown_provider() {
    let (svc, _) = service(
        Arc::new(InMemoryAdmins::default()),
        Arc::new(InMemoryIdentities::default()),
        identity("admin@example.com"),
    );
    assert!(matches!(
        svc.begin("facebook").await,
        Err(OAuthError::Config(_))
    ));
}

#[tokio::test]
async fn complete_links_by_email_then_authenticates() {
    let admins = Arc::new(InMemoryAdmins::default());
    let admin_id = seed_admin(&admins, "admin@example.com", Role::admin());
    let identities = Arc::new(InMemoryIdentities::default());
    let (svc, _) = service(admins, identities.clone(), identity("admin@example.com"));

    let authenticated = sign_in(&svc).await.unwrap();
    assert_eq!(authenticated.id, admin_id);
    assert_eq!(authenticated.role, Role::admin());

    // The identity is now linked for next time.
    assert_eq!(
        identities
            .find_admin(&ProviderId::parse("google").unwrap(), "sub-1")
            .await
            .unwrap(),
        Some(admin_id)
    );
}

#[tokio::test]
async fn complete_rejects_a_replayed_state() {
    let admins = Arc::new(InMemoryAdmins::default());
    seed_admin(&admins, "admin@example.com", Role::admin());
    let (svc, _) = service(
        admins,
        Arc::new(InMemoryIdentities::default()),
        identity("admin@example.com"),
    );

    sign_in(&svc).await.unwrap();
    // The state was consumed; a second use finds nothing.
    assert!(matches!(
        svc.complete("state-123", "the-code", None).await,
        Err(OAuthError::InvalidState)
    ));
}

#[tokio::test]
async fn complete_refuses_an_identity_with_no_matching_admin() {
    let (svc, _) = service(
        Arc::new(InMemoryAdmins::default()),
        Arc::new(InMemoryIdentities::default()),
        identity("stranger@example.com"),
    );
    assert!(matches!(
        sign_in(&svc).await,
        Err(OAuthError::NoLinkedAccount)
    ));
}

#[tokio::test]
async fn complete_never_links_by_an_unverified_email() {
    let admins = Arc::new(InMemoryAdmins::default());
    seed_admin(&admins, "admin@example.com", Role::admin());
    let identities = Arc::new(InMemoryIdentities::default());
    let (svc, _) = service(
        admins,
        identities.clone(),
        OAuthIdentity {
            email_verified: false,
            picture: None,
            ..identity("admin@example.com")
        },
    );
    assert!(matches!(
        sign_in(&svc).await,
        Err(OAuthError::EmailNotVerified)
    ));
    assert!(identities.links.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_linked_identity_signs_in_whatever_email_it_now_carries() {
    let admins = Arc::new(InMemoryAdmins::default());
    let admin_id = seed_admin(&admins, "admin@example.com", Role::admin());
    let identities = Arc::new(InMemoryIdentities::default());
    identities
        .link(
            &ProviderId::parse("google").unwrap(),
            "sub-1",
            None,
            &admin_id,
        )
        .await
        .unwrap();
    // A different (and unverified) email: the link by subject still wins.
    let (svc, _) = service(
        admins,
        identities,
        OAuthIdentity {
            email_verified: false,
            picture: None,
            ..identity("personal@gmail.com")
        },
    );
    assert_eq!(sign_in(&svc).await.unwrap().id, admin_id);
}

#[tokio::test]
async fn a_link_flow_links_any_email_to_the_admin_who_started_it() {
    let admins = Arc::new(InMemoryAdmins::default());
    let admin_id = seed_admin(&admins, "admin@example.com", Role::admin());
    let identities = Arc::new(InMemoryIdentities::default());
    let (svc, _) = service(
        admins,
        identities.clone(),
        OAuthIdentity {
            email_verified: false,
            picture: None,
            ..identity("personal@gmail.com")
        },
    );

    let started = svc.begin_link("google", admin_id.clone()).await.unwrap();
    let outcome = svc
        .complete(&started.state, "the-code", Some(&started.state))
        .await
        .unwrap();
    assert!(
        matches!(outcome, CompleteOutcome::Linked { ref admin_id, .. } if admin_id.as_str() == "id-admin@example.com")
    );
    let linked = svc.linked_identities(&admin_id).await.unwrap();
    assert_eq!(linked.len(), 1);
    assert_eq!(linked[0].subject, "sub-1");

    // And from now on it signs in.
    assert_eq!(sign_in(&svc).await.unwrap().id, admin_id);
}

#[tokio::test]
async fn a_link_flow_must_finish_in_the_browser_that_started_it() {
    let admins = Arc::new(InMemoryAdmins::default());
    let admin_id = seed_admin(&admins, "admin@example.com", Role::admin());
    let identities = Arc::new(InMemoryIdentities::default());
    let (svc, _) = service(admins, identities.clone(), identity("victim@example.com"));

    let started = svc.begin_link("google", admin_id).await.unwrap();
    assert!(matches!(
        svc.complete(&started.state, "the-code", None).await,
        Err(OAuthError::LinkNotBound)
    ));
    assert!(identities.links.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_link_flow_never_takes_an_identity_from_another_admin() {
    let admins = Arc::new(InMemoryAdmins::default());
    let ana = seed_admin(&admins, "ana@example.com", Role::admin());
    let luis = seed_admin(&admins, "luis@example.com", Role::admin());
    let identities = Arc::new(InMemoryIdentities::default());
    let google = ProviderId::parse("google").unwrap();
    identities.link(&google, "sub-1", None, &ana).await.unwrap();
    let (svc, _) = service(admins, identities.clone(), identity("ana@example.com"));

    let started = svc.begin_link("google", luis).await.unwrap();
    assert!(matches!(
        svc.complete(&started.state, "the-code", Some(&started.state))
            .await,
        Err(OAuthError::LinkedToAnotherAccount)
    ));
    assert_eq!(
        identities.find_admin(&google, "sub-1").await.unwrap(),
        Some(ana)
    );
}

#[tokio::test]
async fn unlink_removes_only_ones_own_link() {
    let admins = Arc::new(InMemoryAdmins::default());
    let ana = seed_admin(&admins, "ana@example.com", Role::admin());
    let luis = seed_admin(&admins, "luis@example.com", Role::admin());
    let identities = Arc::new(InMemoryIdentities::default());
    let google = ProviderId::parse("google").unwrap();
    identities.link(&google, "sub-1", None, &ana).await.unwrap();
    let (svc, _) = service(admins, identities, identity("ana@example.com"));

    assert!(!svc.unlink(&luis, "google", "sub-1").await.unwrap());
    assert!(svc.unlink(&ana, "google", "sub-1").await.unwrap());
    assert!(svc.linked_identities(&ana).await.unwrap().is_empty());
}

#[derive(Default)]
struct Outbox(Mutex<Vec<crate::OutgoingMail>>);

#[async_trait::async_trait]
impl crate::Mailer for Outbox {
    async fn send(&self, mail: &crate::OutgoingMail) -> Result<(), String> {
        self.0.lock().unwrap().push(mail.clone());
        Ok(())
    }
}

#[tokio::test]
async fn the_account_hears_when_a_provider_is_linked_or_unlinked() {
    let admins = Arc::new(InMemoryAdmins::default());
    let ana = seed_admin(&admins, "ana@example.com", Role::admin());
    let (svc, _) = service(
        admins,
        Arc::new(InMemoryIdentities::default()),
        identity("ana@example.com"),
    );
    let outbox = Arc::new(Outbox::default());
    let svc = svc.with_mail(MailSlot::fixed(outbox.clone()));

    // La primera entrada la vincula por el correo; la segunda ya no avisa.
    for _ in 0..2 {
        svc.begin("google").await.unwrap();
        svc.complete("state-123", "the-code", None).await.unwrap();
    }
    assert!(svc.unlink(&ana, "google", "sub-1").await.unwrap());
    for _ in 0..50 {
        if outbox.0.lock().unwrap().len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    tokio::time::sleep(Duration::from_millis(10)).await;
    let mut subjects: Vec<String> = outbox
        .0
        .lock()
        .unwrap()
        .iter()
        .map(|m| m.subject.clone())
        .collect();
    subjects.sort();
    assert_eq!(
        subjects,
        [
            "Se desvinculó Google de tu cuenta de Quality Data",
            "Se vinculó Google a tu cuenta de Quality Data",
        ]
    );
}

#[tokio::test]
async fn complete_rejects_an_unknown_state() {
    let (svc, _) = service(
        Arc::new(InMemoryAdmins::default()),
        Arc::new(InMemoryIdentities::default()),
        identity("admin@example.com"),
    );
    assert!(matches!(
        svc.complete("never-issued", "the-code", None).await,
        Err(OAuthError::InvalidState)
    ));
}
