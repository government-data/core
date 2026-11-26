//! The machines' use cases over memory: the rules are the use cases' and
//! the domain's; the SQL ones are in `infrastructure/tests/machines_it.rs`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;
use auth_gateway::MachineRole::{TrinoCoordinator, Worker};
use auth_gateway::MachineState::{Approved, Draining, Retired};
use auth_gateway::{CertificateTerm, MachineChange, MachineId, SignedCertificate, SigningRefusal};
use domain::AuditEventType::{MachineApproved, MachineDraining, MachineRetired};
use domain::{AuditEvent, AuditEventType, AuditId, NewActionAudit, NewAuditEvent};

use super::*;

/// hash, role, pool, expires_at, used, node.
type TokenRow = (
    String,
    MachineRole,
    Option<String>,
    SystemTime,
    bool,
    Option<String>,
);

#[derive(Default)]
struct Tokens {
    rows: Vec<TokenRow>,
}

#[derive(Default)]
struct Memory {
    tokens: Mutex<Tokens>,
    machines: Mutex<Vec<Machine>>,
    /// How many times a machine was looked up by name.
    finds: AtomicU64,
}

#[async_trait]
impl MachineRegistry for Memory {
    async fn issue_token(&self, t: &NewJoinToken) -> Result<String, RepositoryError> {
        let mut tokens = self.tokens.lock().unwrap();
        let row = (
            t.token_hash.clone(),
            t.role,
            t.pool.clone(),
            t.expires_at,
            false,
            t.node.clone(),
        );
        tokens.rows.push(row);
        Ok(format!("t{}", tokens.rows.len()))
    }

    async fn redeem(
        &self,
        hash: &str,
        name: &str,
        now: SystemTime,
    ) -> Result<Result<Machine, RedeemRefusal>, RepositoryError> {
        let mut tokens = self.tokens.lock().unwrap();
        let Some(row) = tokens
            .rows
            .iter_mut()
            .find(|r| r.0 == hash && !r.4 && r.3 > now)
        else {
            return Ok(Err(RedeemRefusal::Token));
        };
        let mut machines = self.machines.lock().unwrap();
        if machines
            .iter()
            .any(|m| m.name == name && m.state != MachineState::Retired)
        {
            return Ok(Err(RedeemRefusal::NameTaken));
        }
        if row.5.as_deref().is_some_and(|node| node != name) {
            return Ok(Err(RedeemRefusal::NodeName));
        }
        row.4 = true;
        let machine = Machine {
            id: MachineId::new(format!("m{}", machines.len() + 1)),
            name: name.into(),
            role: row.1,
            pool: row.2.clone(),
            state: MachineState::Pending,
            version: None,
            cert_serial: None,
            cert_not_after: None,
            cert_serial_next: None,
            cert_not_after_next: None,
            created_at: now,
            approved_at: None,
            approved_by: None,
            retired_at: None,
            retired_by: None,
        };
        machines.push(machine.clone());
        Ok(Ok(machine))
    }

    async fn create_approved(
        &self,
        name: &str,
        role: MachineRole,
        now: SystemTime,
    ) -> Result<Option<Machine>, RepositoryError> {
        let mut machines = self.machines.lock().unwrap();
        let live = |m: &Machine| m.name == name && m.state != MachineState::Retired;
        if machines.iter().any(live) {
            return Ok(None);
        }
        let machine = Machine {
            id: MachineId::new(format!("m{}", machines.len() + 1)),
            name: name.into(),
            role,
            pool: None,
            state: MachineState::Approved,
            version: None,
            cert_serial: None,
            cert_not_after: None,
            cert_serial_next: None,
            cert_not_after_next: None,
            created_at: now,
            approved_at: Some(now),
            approved_by: None,
            retired_at: None,
            retired_by: None,
        };
        machines.push(machine.clone());
        Ok(Some(machine))
    }

    async fn transition(
        &self,
        id: &MachineId,
        to: MachineState,
        _by: &AdminId,
        _now: SystemTime,
    ) -> Result<MachineChange, RepositoryError> {
        let mut machines = self.machines.lock().unwrap();
        let Some(m) = machines.iter_mut().find(|m| &m.id == id) else {
            return Ok(MachineChange::NotFound);
        };
        if !m.state.may_become(to) {
            return Ok(MachineChange::NotAllowed(m.state));
        }
        m.state = to;
        Ok(MachineChange::Done(Box::new(m.clone())))
    }

    async fn record_certificate(
        &self,
        id: &MachineId,
        serial: &str,
        not_after: SystemTime,
    ) -> Result<MachineChange, RepositoryError> {
        let mut machines = self.machines.lock().unwrap();
        let Some(m) = machines.iter_mut().find(|m| &m.id == id) else {
            return Ok(MachineChange::NotFound);
        };
        if m.state == MachineState::Retired {
            return Ok(MachineChange::NotAllowed(m.state));
        }
        m.cert_serial = Some(serial.into());
        m.cert_not_after = Some(not_after);
        (m.cert_serial_next, m.cert_not_after_next) = (None, None);
        Ok(MachineChange::Done(Box::new(m.clone())))
    }

    async fn record_next_certificate(
        &self,
        id: &MachineId,
        serial: &str,
        not_after: SystemTime,
    ) -> Result<MachineChange, RepositoryError> {
        let mut machines = self.machines.lock().unwrap();
        let Some(m) = machines.iter_mut().find(|m| &m.id == id) else {
            return Ok(MachineChange::NotFound);
        };
        if m.state == MachineState::Retired {
            return Ok(MachineChange::NotAllowed(m.state));
        }
        m.cert_serial_next = Some(serial.into());
        m.cert_not_after_next = Some(not_after);
        Ok(MachineChange::Done(Box::new(m.clone())))
    }

    async fn promote_next_certificate(
        &self,
        id: &MachineId,
        serial: &str,
    ) -> Result<MachineChange, RepositoryError> {
        let mut machines = self.machines.lock().unwrap();
        let Some(m) = machines.iter_mut().find(|m| &m.id == id) else {
            return Ok(MachineChange::NotFound);
        };
        if m.state == MachineState::Retired || m.cert_serial_next.as_deref() != Some(serial) {
            return Ok(MachineChange::NotAllowed(m.state));
        }
        m.cert_serial = m.cert_serial_next.take();
        m.cert_not_after = m.cert_not_after_next.take();
        Ok(MachineChange::Done(Box::new(m.clone())))
    }

    async fn find(&self, id: &MachineId) -> Result<Option<Machine>, RepositoryError> {
        let machines = self.machines.lock().unwrap();
        Ok(machines.iter().find(|m| &m.id == id).cloned())
    }

    async fn find_live(&self, name: &str) -> Result<Option<Machine>, RepositoryError> {
        self.finds.fetch_add(1, Ordering::SeqCst);
        let machines = self.machines.lock().unwrap();
        let live = |m: &&Machine| m.name == name && m.state != MachineState::Retired;
        Ok(machines.iter().find(live).cloned())
    }

    async fn list(&self) -> Result<Vec<Machine>, RepositoryError> {
        Ok(self.machines.lock().unwrap().clone())
    }
}

/// Tokens of the right form, numbered; the "hash" is the token reversed.
#[derive(Default)]
struct Mint(AtomicU64);

impl JoinTokenMint for Mint {
    fn mint(&self) -> Option<String> {
        let n = self.0.fetch_add(1, Ordering::SeqCst);
        Some(format!("qdj_{n:0>43}"))
    }

    fn hash(&self, token: &str) -> String {
        token.chars().rev().collect()
    }
}

#[derive(Default)]
struct Audit(Mutex<Vec<NewActionAudit>>);

#[async_trait]
impl AuditRepository for Audit {
    async fn record(&self, _: &NewAuditEvent) -> Result<AuditId, RepositoryError> {
        Ok(AuditId::new("a"))
    }

    async fn record_action(&self, event: &NewActionAudit) -> Result<AuditId, RepositoryError> {
        self.0.lock().unwrap().push(event.clone());
        Ok(AuditId::new("a"))
    }

    async fn list_recent(&self, _: u32) -> Result<Vec<AuditEvent>, RepositoryError> {
        Ok(Vec::new())
    }
}

struct FixedClock(Mutex<SystemTime>);

impl Clock for FixedClock {
    fn now(&self) -> SystemTime {
        *self.0.lock().unwrap()
    }
}

/// Signs anything but `bad`; with `fails`, signing itself fails.
#[derive(Default)]
struct Signer {
    fails: bool,
}

const DAY: Duration = Duration::from_secs(24 * 3600);

impl CertificateSigner for Signer {
    fn check(&self, csr: &str, _: &str) -> Result<(), SigningRefusal> {
        match csr {
            "bad" => Err(SigningRefusal::Request("bad".into())),
            _ => Ok(()),
        }
    }

    fn sign(
        &self,
        _: &str,
        role: MachineRole,
        name: &str,
        term: CertificateTerm,
    ) -> Result<SignedCertificate, SigningRefusal> {
        if self.fails {
            return Err(SigningRefusal::Failed("no randomness".into()));
        }
        let days = match term {
            CertificateTerm::Day => 1,
            CertificateTerm::UntilReplaced => 365,
        };
        Ok(SignedCertificate {
            pem: format!("cert {} {name}", role.as_str()),
            serial: "AB12".into(),
            not_after: SystemTime::UNIX_EPOCH + DAY * days,
        })
    }

    fn ca_pem(&self) -> String {
        "ca".into()
    }
}

struct World {
    service: MachinesService,
    audit: Arc<Audit>,
    clock: Arc<FixedClock>,
}

fn world() -> World {
    world_with(Signer::default())
}

fn world_with(signer: Signer) -> World {
    let audit = Arc::new(Audit::default());
    let clock = Arc::new(FixedClock(Mutex::new(SystemTime::UNIX_EPOCH)));
    let service = MachinesService::new(
        Arc::new(Memory::default()),
        Arc::new(Mint::default()),
        audit.clone(),
        clock.clone(),
    )
    .with_signer(Arc::new(signer));
    World {
        service,
        audit,
        clock,
    }
}

fn origin() -> Origin {
    Origin {
        ip: "192.0.2.10".into(),
        user_agent: None,
    }
}

fn admin() -> AdminId {
    AdminId::new("00000000-0000-0000-0000-00000000000a")
}

impl World {
    fn kinds(&self) -> Vec<AuditEventType> {
        let events = self.audit.0.lock().unwrap();
        events.iter().map(|e| e.event_type).collect()
    }

    fn advance(&self, by: Duration) {
        *self.clock.0.lock().unwrap() += by;
    }

    async fn joined(&self, name: &str) -> Machine {
        let issued = self.issue().await;
        self.service
            .redeem(&issued.token, name, &origin())
            .await
            .unwrap()
    }

    async fn issue(&self) -> IssuedJoinToken {
        let s = &self.service;
        s.issue_join_token(Worker, Some("default"), &admin(), &origin())
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn machines_join_once_with_a_token_of_an_hour() {
    let w = world();
    let issued = w.issue().await;
    assert!(is_join_token(&issued.token));
    assert_eq!(issued.expires_at, SystemTime::UNIX_EPOCH + JOIN_TOKEN_TTL);

    let machine = w
        .service
        .redeem(&issued.token, "worker-01", &origin())
        .await
        .unwrap();
    assert_eq!(machine.state, MachineState::Pending);
    assert_eq!(
        (machine.role, machine.pool.as_deref()),
        (Worker, Some("default"))
    );

    let again = w
        .service
        .redeem(&issued.token, "worker-02", &origin())
        .await;
    assert_eq!(again, Err(JoinError::Refused));

    let late = w.issue().await;
    w.advance(JOIN_TOKEN_TTL);
    let expired = w.service.redeem(&late.token, "worker-03", &origin()).await;
    assert_eq!(expired, Err(JoinError::Refused));
    let unknown = format!("qdj_{:0>43}", 9);
    let unknown = w.service.redeem(&unknown, "w", &origin()).await;
    assert_eq!(unknown, Err(JoinError::Refused));
    let malformed = w.service.redeem("qdj_short", "worker-04", &origin()).await;
    assert_eq!(malformed, Err(JoinError::Refused));

    use AuditEventType::*;
    let kinds = [
        JoinTokenIssued,
        JoinTokenUsed,
        JoinTokenRefused,
        JoinTokenIssued,
    ];
    let refused = [JoinTokenRefused; 3];
    assert_eq!(w.kinds(), [&kinds[..], &refused[..]].concat());
}

#[tokio::test]
async fn machines_audit_never_holds_the_token_or_its_hash() {
    let w = world();
    let issued = w.issue().await;
    w.service
        .redeem(&issued.token, "worker-01", &origin())
        .await
        .unwrap();
    let hash: String = issued.token.chars().rev().collect();
    let events = w.audit.0.lock().unwrap();
    for e in events.iter() {
        let text = format!("{e:?}");
        assert!(
            !text.contains(&issued.token) && !text.contains(&hash),
            "{text}"
        );
    }
    assert_eq!(events[0].action, "join:worker/default");
    assert_eq!(events[0].admin_id, Some(admin()));
    assert_eq!(events[1].service_account.as_deref(), Some("worker-01"));
    assert_eq!(events[1].resource, "machine/worker-01");
}

#[tokio::test]
async fn machines_refuse_bad_names_and_names_in_use() {
    let w = world();
    let first = w.joined("worker-01").await;
    let issued = w.issue().await;
    let taken = w
        .service
        .redeem(&issued.token, "worker-01", &origin())
        .await;
    assert_eq!(taken, Err(JoinError::NameTaken));
    let bad = w.service.redeem(&issued.token, "Worker_1", &origin()).await;
    assert_eq!(bad, Err(JoinError::InvalidName));
    {
        let events = w.audit.0.lock().unwrap();
        let actions: Vec<_> = events.iter().rev().take(2).map(|e| &e.action).collect();
        assert_eq!(actions, ["join:invalid-name", "join:name-taken"]);
        assert_eq!(
            events.last().map(|e| e.resource.as_str()),
            Some("machine/-")
        );
    }

    // The token was not used by the refusal; a retired name joins again.
    w.service
        .retire(&first.id, &admin(), &origin())
        .await
        .unwrap();
    let again = w
        .service
        .redeem(&issued.token, "worker-01", &origin())
        .await;
    assert_eq!(again.unwrap().state, MachineState::Pending);
}

#[tokio::test]
async fn machines_pools_are_labels() {
    let w = world();
    let bad = w
        .service
        .issue_join_token(Worker, Some("Default pool"), &admin(), &origin())
        .await;
    assert_eq!(bad, Err(MachinesError::Invalid("pool")));
    assert!(w.kinds().is_empty());
}

#[tokio::test]
async fn machines_of_the_application_vm_do_not_join() {
    let w = world();
    let app = auth_gateway::MachineRole::App;
    let (by, o) = (admin(), origin());
    let refused = w.service.issue_join_token(app, None, &by, &o).await;
    assert_eq!(refused, Err(MachinesError::Invalid("role")));
    assert!(w.kinds().is_empty(), "no token, nothing audited");
}

#[tokio::test]
async fn machines_move_forward_only_and_are_audited() {
    let w = world();
    let m = w.joined("worker-01").await;
    let (s, by, o) = (&w.service, admin(), origin());
    assert_eq!(
        s.drain(&m.id, &by, &o).await,
        Err(MachinesError::NotAllowed(MachineState::Pending))
    );
    assert_eq!(s.approve(&m.id, &by, &o).await.unwrap().state, Approved);
    assert_eq!(
        s.approve(&m.id, &by, &o).await,
        Err(MachinesError::NotAllowed(Approved))
    );
    assert_eq!(s.drain(&m.id, &by, &o).await.unwrap().state, Draining);
    assert_eq!(s.retire(&m.id, &by, &o).await.unwrap().state, Retired);
    for refused in [
        s.approve(&m.id, &by, &o).await,
        s.drain(&m.id, &by, &o).await,
        s.retire(&m.id, &by, &o).await,
    ] {
        assert_eq!(refused, Err(MachinesError::NotAllowed(Retired)));
    }
    let unknown = s.approve(&MachineId::new("none"), &by, &o).await;
    assert_eq!(unknown, Err(MachinesError::NotFound));
    let kinds = w.kinds();
    assert_eq!(
        &kinds[2..],
        [MachineApproved, MachineDraining, MachineRetired]
    );
}

#[tokio::test]
async fn machines_accept_only_the_serial_in_use() {
    let w = world();
    let m = w.joined("worker-01").await;
    let (s, o) = (&w.service, origin());
    let end = SystemTime::UNIX_EPOCH + Duration::from_secs(86_400);
    assert_eq!(s.accepts("worker-01", "0a").await, Ok(Acceptance::Refused));

    let recorded = s.record_certificate(&m.id, "0A1B", end, &o).await.unwrap();
    assert_eq!(recorded.cert_serial.as_deref(), Some("0a1b"));
    let Ok(Acceptance::Accepted(pending)) = s.accepts("worker-01", "0a1b").await else {
        panic!("the serial in use is accepted");
    };
    assert_eq!(pending.state, MachineState::Pending);

    s.record_certificate(&m.id, "ff", end, &o).await.unwrap();
    assert_eq!(
        s.accepts("worker-01", "0a1b").await,
        Ok(Acceptance::Refused)
    );
    assert!(matches!(
        s.accepts("worker-01", "ff").await,
        Ok(Acceptance::Accepted(_))
    ));
    assert_eq!(s.accepts("worker-02", "ff").await, Ok(Acceptance::Refused));

    s.retire(&m.id, &admin(), &o).await.unwrap();
    assert_eq!(s.accepts("worker-01", "ff").await, Ok(Acceptance::Refused));
    assert_eq!(
        s.record_certificate(&m.id, "ee", end, &o).await,
        Err(MachinesError::NotAllowed(Retired))
    );
    for bad in ["", "xyz", &"a".repeat(65)] {
        let refused = s.record_certificate(&m.id, bad, end, &o).await;
        assert_eq!(refused, Err(MachinesError::Invalid("serial")));
    }
    let issues = w
        .kinds()
        .into_iter()
        .filter(|k| *k == AuditEventType::CertificateIssued);
    assert_eq!(issues.count(), 2);
}

#[tokio::test]
async fn machines_are_listed() {
    let w = world();
    w.joined("worker-01").await;
    w.joined("worker-02").await;
    let names: Vec<_> = w.service.list().await.unwrap();
    let names: Vec<_> = names.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["worker-01", "worker-02"]);
}

#[tokio::test]
async fn machines_join_with_a_certificate_for_their_role() {
    let (o, a) = (origin(), admin());
    let w = world();
    let issued = w.issue().await;
    let joined = w
        .service
        .join(&issued.token, "worker-01", "csr", &o)
        .await
        .unwrap();
    assert_eq!(joined.certificate, "cert worker worker-01");
    assert_eq!((joined.ca.as_str(), joined.serial.as_str()), ("ca", "AB12"));
    assert_eq!(joined.not_after, SystemTime::UNIX_EPOCH + DAY);
    assert_eq!(joined.machine.state, MachineState::Pending);
    assert_eq!(joined.machine.cert_serial.as_deref(), Some("ab12"));
    let accepted = w.service.accepts("worker-01", "ab12").await.unwrap();
    assert!(matches!(accepted, Acceptance::Accepted(_)));

    // Trino's coordinator takes its certificate only at a restart.
    let s = &w.service;
    let trino = s.issue_join_token(TrinoCoordinator, None, &a, &o);
    let trino = trino.await.unwrap();
    let joined = s.join(&trino.token, "trino-1", "csr", &o).await;
    let year = SystemTime::UNIX_EPOCH + DAY * 365;
    assert_eq!(joined.unwrap().not_after, year);
    use AuditEventType::*;
    let one = [JoinTokenIssued, JoinTokenUsed, CertificateIssued];
    assert_eq!(w.kinds(), [one, one].concat());
}

#[tokio::test]
async fn machines_with_a_bad_request_leave_the_token_unused() {
    let o = origin();
    let w = world();
    let issued = w.issue().await;
    let bad = w.service.join(&issued.token, "worker-01", "bad", &o);
    assert_eq!(bad.await, Err(JoinError::InvalidRequest));
    let refused = w.audit.0.lock().unwrap().last().unwrap().action.clone();
    assert_eq!(refused, "join:invalid-request");
    let good = w.service.join(&issued.token, "worker-01", "csr", &o);
    assert!(good.await.is_ok());
    let bad_name = w.service.join(&issued.token, "Worker", "csr", &o);
    assert_eq!(bad_name.await, Err(JoinError::InvalidName));
}

#[tokio::test]
async fn machines_signed_for_nothing_are_left_pending_without_one() {
    let o = origin();
    let w = world_with(Signer { fails: true });
    let issued = w.issue().await;
    let failed = w.service.join(&issued.token, "worker-01", "csr", &o);
    assert!(matches!(failed.await, Err(JoinError::Backend(_))));
    // The token is used; the machine is there, with no certificate.
    let again = w.service.join(&issued.token, "worker-02", "csr", &o);
    assert_eq!(again.await, Err(JoinError::Refused));
    let listed = w.service.list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].cert_serial, None);
}

#[tokio::test]
async fn machines_cannot_join_without_a_ca() {
    let (o, a) = (origin(), admin());
    let audit = Arc::new(Audit::default());
    let clock = Arc::new(FixedClock(Mutex::new(SystemTime::UNIX_EPOCH)));
    let memory = Arc::new(Memory::default());
    let service = MachinesService::new(memory, Arc::new(Mint::default()), audit.clone(), clock);
    assert!(!service.can_join());
    let issued = service.issue_join_token(Worker, None, &a, &o);
    assert_eq!(issued.await, Err(MachinesError::Unavailable));
    let token = format!("qdj_{:0>43}", 1);
    let joined = service.join(&token, "worker-01", "csr", &o).await;
    assert_eq!(joined, Err(JoinError::Unavailable));
    assert!(audit.0.lock().unwrap().is_empty());
}

/// What Traefik would pass for `name`'s certificate of `unit` and `serial`.
fn presented(unit: &str, name: &str, serial: &str) -> auth_gateway::MachineCertificate {
    auth_gateway::MachineCertificate {
        organization: Some("qd".into()),
        unit: Some(unit.into()),
        common_name: Some(name.into()),
        serial: serial.into(),
        not_after: SystemTime::UNIX_EPOCH,
    }
}

#[tokio::test]
async fn machines_renew_into_a_next_certificate_used_once_presented() {
    let w = world();
    let m = w.joined("worker-01").await;
    let (s, o) = (&w.service, origin());
    let end = SystemTime::UNIX_EPOCH + DAY;
    s.record_certificate(&m.id, "0a", end, &o).await.unwrap();

    let old = presented("worker", "worker-01", "0a");
    let renewed = s.renew(&old, "csr", &o).await.unwrap();
    assert_eq!(
        (renewed.serial.as_str(), renewed.ca.as_str()),
        ("ab12", "ca")
    );
    assert_eq!(renewed.machine.cert_serial.as_deref(), Some("0a"));
    assert_eq!(renewed.machine.cert_serial_next.as_deref(), Some("ab12"));
    // Both are accepted until the next one is presented; then only it.
    assert!(matches!(
        s.accepts("worker-01", "0a").await,
        Ok(Acceptance::Accepted(_))
    ));
    let Ok(Acceptance::Accepted(now)) = s.accepts("worker-01", "AB12").await else {
        panic!("the next one is accepted");
    };
    assert_eq!(
        (now.cert_serial.as_deref(), now.cert_serial_next),
        (Some("ab12"), None)
    );
    assert_eq!(s.accepts("worker-01", "0a").await, Ok(Acceptance::Refused));

    let refused = |unit, name, serial| {
        let presented = presented(unit, name, serial);
        async move { s.renew(&presented, "csr", &origin()).await }
    };
    assert_eq!(
        refused("worker", "worker-01", "0a").await,
        Err(RenewError::Refused)
    );
    assert_eq!(
        refused("lake", "worker-01", "ab12").await,
        Err(RenewError::Refused)
    );
    assert_eq!(
        refused("worker", "worker-09", "ab12").await,
        Err(RenewError::Refused)
    );
    let new = presented("worker", "worker-01", "ab12");
    let bad = s.renew(&new, "bad", &o).await;
    assert_eq!(bad, Err(RenewError::InvalidRequest));
    s.renew_without_certificate(&o).await;
    let events = w.audit.0.lock().unwrap();
    let actions: Vec<_> = events.iter().map(|e| e.action.as_str()).collect();
    assert_eq!(
        &actions[actions.len() - 6..],
        [
            "renew:worker",
            "renew:certificate",
            "renew:role",
            "renew:certificate",
            "renew:invalid-request",
            "renew:no-certificate"
        ]
    );
}

/// The queue's sessions: ends two for each machine, or fails.
#[derive(Default)]
struct Sessions {
    ended: Mutex<Vec<String>>,
    fails: bool,
}

#[async_trait]
impl MachineSessions for Sessions {
    async fn end(&self, name: &str) -> Result<u64, RepositoryError> {
        if self.fails {
            return Err(RepositoryError::Backend("no privilege".into()));
        }
        self.ended.lock().unwrap().push(name.into());
        Ok(2)
    }
}

#[tokio::test]
async fn retiring_ends_the_machines_sessions_and_a_failure_keeps_it_retired() {
    for fails in [false, true] {
        let sessions = Arc::new(Sessions {
            fails,
            ..Sessions::default()
        });
        let mut w = world();
        w.service = w.service.with_sessions(sessions.clone());
        let m = w.joined("worker-01").await;
        let (s, by, o) = (&w.service, admin(), origin());
        s.approve(&m.id, &by, &o).await.unwrap();
        s.drain(&m.id, &by, &o).await.unwrap();
        assert!(sessions.ended.lock().unwrap().is_empty(), "only retiring");

        assert_eq!(s.retire(&m.id, &by, &o).await.unwrap().state, Retired);
        let ended = sessions.ended.lock().unwrap().clone();
        let events = w.audit.0.lock().unwrap().clone();
        let last = events.last().unwrap();
        assert_eq!(last.event_type, AuditEventType::MachineSessionsEnded);
        assert_eq!(last.service_account.as_deref(), Some("worker-01"));
        if fails {
            assert!(ended.is_empty());
            assert_eq!(last.action, "sessions:failed");
        } else {
            assert_eq!(ended, ["worker-01"]);
            assert_eq!(last.action, "sessions:2");
        }
        assert_eq!(events[events.len() - 2].event_type, MachineRetired);
        assert_eq!(
            s.retire(&m.id, &by, &o).await,
            Err(MachinesError::NotAllowed(Retired)),
            "a refused retirement ends nothing more"
        );
        assert_eq!(sessions.ended.lock().unwrap().len(), ended.len());
    }
}

mod alerts;
mod dolt;
mod install;
mod install_files;
mod lake;
mod lake_adapters;
mod lake_admin;
mod lake_keys;
mod lake_pools;
mod lake_replica;
mod lake_replication;
mod lake_sync;
mod lake_users;
mod lake_warehouse;
mod series;
