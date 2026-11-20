//! A machine role's client (0066, point 6): a token for the certificate of
//! an approved or draining machine of the role, 15 minutes, naming it; every
//! other case `invalid_client`.

use super::*;
use crate::domain::{
    Machine, MachineCertificate, MachineChange, MachineId, MachineRegistry, MachineRole,
    MachineState, NewJoinToken, RedeemRefusal,
};

/// The machines by name; promotes a next serial as PostgreSQL does.
#[derive(Default)]
struct Machines(Mutex<HashMap<String, Machine>>);

#[async_trait::async_trait]
impl MachineRegistry for Machines {
    async fn issue_token(&self, _: &NewJoinToken) -> Result<String, RepositoryError> {
        unreachable!()
    }
    async fn redeem(
        &self,
        _: &str,
        _: &str,
        _: SystemTime,
    ) -> Result<Result<Machine, RedeemRefusal>, RepositoryError> {
        unreachable!()
    }
    async fn create_approved(
        &self,
        _: &str,
        _: MachineRole,
        _: SystemTime,
    ) -> Result<Option<Machine>, RepositoryError> {
        unreachable!()
    }
    async fn transition(
        &self,
        _: &MachineId,
        _: MachineState,
        _: &AdminId,
        _: SystemTime,
    ) -> Result<MachineChange, RepositoryError> {
        unreachable!()
    }
    async fn record_certificate(
        &self,
        _: &MachineId,
        _: &str,
        _: SystemTime,
    ) -> Result<MachineChange, RepositoryError> {
        unreachable!()
    }
    async fn record_next_certificate(
        &self,
        _: &MachineId,
        _: &str,
        _: SystemTime,
    ) -> Result<MachineChange, RepositoryError> {
        unreachable!()
    }
    async fn promote_next_certificate(
        &self,
        id: &MachineId,
        serial: &str,
    ) -> Result<MachineChange, RepositoryError> {
        let mut all = self.0.lock().unwrap();
        let Some(m) = all.values_mut().find(|m| &m.id == id) else {
            return Ok(MachineChange::NotFound);
        };
        if m.cert_serial_next.as_deref() != Some(serial) {
            return Ok(MachineChange::NotAllowed(m.state));
        }
        m.cert_serial = m.cert_serial_next.take();
        Ok(MachineChange::Done(Box::new(m.clone())))
    }
    async fn find(&self, _: &MachineId) -> Result<Option<Machine>, RepositoryError> {
        unreachable!()
    }
    async fn find_live(&self, name: &str) -> Result<Option<Machine>, RepositoryError> {
        Ok(self.0.lock().unwrap().get(name).cloned())
    }
    async fn list(&self) -> Result<Vec<Machine>, RepositoryError> {
        unreachable!()
    }
}

fn machine(name: &str, role: MachineRole, state: MachineState) -> Machine {
    Machine {
        id: MachineId::new(format!("id-{name}")),
        name: name.into(),
        role,
        pool: None,
        state,
        version: None,
        cert_serial: Some("aa01".into()),
        cert_not_after: None,
        cert_serial_next: Some("bb02".into()),
        cert_not_after_next: None,
        created_at: SystemTime::UNIX_EPOCH,
        approved_at: None,
        approved_by: None,
        retired_at: None,
        retired_by: None,
    }
}

fn cert(unit: &str, name: &str, serial: &str) -> MachineCertificate {
    MachineCertificate {
        organization: Some("qd".into()),
        unit: Some(unit.into()),
        common_name: Some(name.into()),
        serial: serial.into(),
        not_after: SystemTime::UNIX_EPOCH,
    }
}

/// `gestion` as the workers' client, and one machine of each state.
fn workers() -> Fixture {
    use MachineState::*;
    let settings = ClientSettings {
        client_credentials: true,
        audiences: vec!["quality-worker".into()],
        ..ClientSettings::default()
    };
    let role = MachineRole::Worker;
    let mut f = fixture_with(ClientAuth::Certificate { role }, settings);
    let machines = Machines::default();
    for (name, role, state) in [
        ("w-approved", role, Approved),
        ("w-draining", role, Draining),
        ("w-pending", role, Pending),
        ("w-retired", role, Retired),
        ("t-coordinator", MachineRole::TrinoCoordinator, Approved),
    ] {
        let m = machine(name, role, state);
        machines.0.lock().unwrap().insert(name.into(), m);
    }
    f.server = f.server.with_machines(Arc::new(machines));
    f
}

fn asking(certificate: Option<MachineCertificate>) -> TokenRequest {
    TokenRequest {
        grant_type: Some("client_credentials".into()),
        client_id: Some("gestion".into()),
        audience: Some("quality-worker".into()),
        client_certificate: certificate,
        ..TokenRequest::default()
    }
}

#[tokio::test]
async fn an_approved_or_draining_machine_gets_15_minutes_that_name_it() {
    let f = workers();
    for name in ["w-approved", "w-draining"] {
        let req = asking(Some(cert("worker", name, "AA01")));
        let token = f.server.token(&req).await.unwrap();
        assert_eq!(token.expires_in, 900);
        let c = claims(&token.access_token);
        assert_eq!(c["exp"].as_u64().unwrap() - c["iat"].as_u64().unwrap(), 900);
        assert_eq!(
            (c["machine"].as_str(), c["sub"].as_str()),
            (Some(name), Some("gestion"))
        );
        let client = f
            .server
            .machine_client(&token.access_token, "quality-worker");
        assert_eq!(client.as_deref(), Some("gestion"));
        // The worker routes read the machine too (0066, step 7).
        let caller = f
            .server
            .machine_caller(&token.access_token, "quality-worker");
        assert_eq!(caller, Some(("gestion".into(), Some(name.into()))));
    }
}

#[tokio::test]
async fn a_machine_role_refuses_everything_else_as_invalid_client() {
    let f = workers();
    let mut other_o = cert("worker", "w-approved", "aa01");
    other_o.organization = Some("evil".into());
    let refused = [
        asking(None),
        asking(Some(cert("worker", "w-pending", "aa01"))),
        asking(Some(cert("worker", "w-retired", "aa01"))),
        asking(Some(cert("worker", "w-unknown", "aa01"))),
        asking(Some(cert("worker", "w-approved", "cc03"))),
        asking(Some(cert("trino-coordinator", "t-coordinator", "aa01"))),
        // A worker's OU on another role's machine.
        asking(Some(cert("worker", "t-coordinator", "aa01"))),
        asking(Some(other_o)),
        TokenRequest {
            client_secret: Some("s".into()),
            ..asking(Some(cert("worker", "w-approved", "aa01")))
        },
    ];
    for req in &refused {
        assert_eq!(
            f.server.token(req).await,
            Err(TokenError::InvalidClient),
            "{req:?}"
        );
    }
    // Never by a code either.
    let code = TokenRequest {
        client_certificate: Some(cert("worker", "w-approved", "aa01")),
        ..token_request("code-1")
    };
    assert_eq!(f.server.token(&code).await, Err(TokenError::InvalidClient));
}

#[tokio::test]
async fn the_next_certificate_is_taken_and_the_one_before_refused() {
    let f = workers();
    let next = asking(Some(cert("worker", "w-approved", "bb02")));
    assert!(f.server.token(&next).await.is_ok());
    assert!(f.server.token(&next).await.is_ok());
    let old = asking(Some(cert("worker", "w-approved", "aa01")));
    assert_eq!(f.server.token(&old).await, Err(TokenError::InvalidClient));
}

#[tokio::test]
async fn without_the_machines_no_certificate_authenticates() {
    let settings = ClientSettings {
        client_credentials: true,
        audiences: vec!["quality-worker".into()],
        ..ClientSettings::default()
    };
    let role = MachineRole::Worker;
    let f = fixture_with(ClientAuth::Certificate { role }, settings);
    let req = asking(Some(cert("worker", "w-approved", "aa01")));
    assert_eq!(f.server.token(&req).await, Err(TokenError::InvalidClient));
}

#[tokio::test]
async fn a_secret_client_keeps_its_day_whatever_certificate_comes() {
    let settings = ClientSettings {
        client_credentials: true,
        audiences: vec!["quality-worker".into()],
        ..ClientSettings::default()
    };
    let secret = ClientAuth::Secret { hash: "h:s".into() };
    let f = fixture_with(secret, settings);
    let req = TokenRequest {
        client_secret: Some("s".into()),
        ..asking(Some(cert("worker", "w-approved", "aa01")))
    };
    let token = f.server.token(&req).await.unwrap();
    assert_eq!(token.expires_in, TOKEN_TTL.as_secs());
    assert!(claims(&token.access_token).get("machine").is_none());
}

#[test]
fn discovery_lists_tls_client_auth() {
    let methods = workers().server.discovery()["token_endpoint_auth_methods_supported"].clone();
    assert!(methods
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m == "tls_client_auth"));
}
