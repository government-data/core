//! What a new machine is installed with (0029, 0066 step 11): the script
//! and the CA for anyone, the role's compose file for a usable join token,
//! which it does not use up, and the command built from the same hashes.

use auth_gateway::{InstallFile, InstallFiles, JoinGrant, JoinTokenCheck};

use crate::machines::Drives;

use super::*;

#[async_trait]
impl JoinTokenCheck for Memory {
    async fn usable(
        &self,
        hash: &str,
        now: SystemTime,
    ) -> Result<Option<JoinGrant>, RepositoryError> {
        let tokens = self.tokens.lock().unwrap();
        let row = tokens
            .rows
            .iter()
            .find(|r| r.0 == hash && !r.4 && r.3 > now);
        Ok(row.map(|r| JoinGrant {
            role: r.1,
            pool: r.2.clone(),
            node: r.5.clone(),
        }))
    }
}

/// A worker's compose file only, and a CA if `with_ca`.
struct Files {
    with_ca: bool,
}

fn file(body: &str, sha256: &str) -> InstallFile {
    InstallFile {
        body: body.into(),
        sha256: sha256.into(),
    }
}

impl InstallFiles for Files {
    fn script(&self) -> InstallFile {
        file("#!/bin/sh", &"5".repeat(64))
    }

    fn compose(&self, role: MachineRole) -> Option<InstallFile> {
        (role == Worker).then(|| file("services: {}", &"c".repeat(64)))
    }

    fn file(&self, role: MachineRole, name: &str) -> Option<InstallFile> {
        (role == Worker && name == "entry.sh").then(|| file("#!/bin/sh", &"e".repeat(64)))
    }

    fn ca(&self) -> Option<InstallFile> {
        self.with_ca.then(|| file("ca", &"a".repeat(64)))
    }
}

pub(super) fn installing(url: &str, with_ca: bool) -> World {
    let memory = Arc::new(Memory::default());
    let audit = Arc::new(Audit::default());
    let clock = Arc::new(FixedClock(Mutex::new(SystemTime::UNIX_EPOCH)));
    let service = MachinesService::new(
        memory.clone(),
        Arc::new(Mint::default()),
        audit.clone(),
        clock.clone(),
    )
    .with_signer(Arc::new(Signer::default()))
    .with_install(Arc::new(Files { with_ca }), memory, url);
    World {
        service,
        audit,
        clock,
    }
}

#[tokio::test]
async fn the_compose_file_is_given_for_a_usable_token_which_stays_usable() {
    let w = installing("https://qd.example/", true);
    let issued = w.issue().await;
    for _ in 0..2 {
        let compose = w.service.role_compose(&issued.token, Worker).await;
        assert_eq!(compose.unwrap().sha256, "c".repeat(64));
    }
    // Not used up: the machine joins with it afterwards, once.
    let from = origin();
    let joined = w.service.join(&issued.token, "worker-7", "csr", &from);
    assert_eq!(joined.await.unwrap().machine.name, "worker-7");
    let used = w.service.role_compose(&issued.token, Worker).await;
    assert_eq!(used, Err(InstallFileError::Refused));
    // Only the issue and the join were audited.
    use AuditEventType::*;
    let kinds = [JoinTokenIssued, JoinTokenUsed, CertificateIssued];
    assert_eq!(w.kinds(), kinds);
}

#[tokio::test]
async fn the_compose_file_is_refused_without_a_usable_token_of_its_role() {
    let w = installing("https://qd.example", true);
    let refused = Err(InstallFileError::Refused);
    let unknown = format!("qdj_{:0>43}", 9);
    for token in ["", "qdj_short", unknown.as_str()] {
        assert_eq!(w.service.role_compose(token, Worker).await, refused);
    }
    let late = w.issue().await;
    w.advance(JOIN_TOKEN_TTL);
    assert_eq!(w.service.role_compose(&late.token, Worker).await, refused);
    // A worker's token is not another role's.
    let issued = w.issue().await;
    let other = w.service.role_compose(&issued.token, TrinoCoordinator);
    assert_eq!(other.await, refused);
    // A role's own token, but no compose file for it yet.
    let s = &w.service;
    let (by, from) = (admin(), origin());
    let trino = s.issue_join_token(MachineRole::TrinoWorker, None, &by, &from);
    let trino = trino.await.unwrap();
    let none = s.role_compose(&trino.token, MachineRole::TrinoWorker).await;
    assert_eq!(none, Err(InstallFileError::NotFound));
    // Without installation files, nothing.
    let bare = world().service.role_compose(&issued.token, Worker).await;
    assert_eq!(bare, Err(InstallFileError::Unavailable));
}

#[tokio::test]
async fn the_files_the_compose_names_are_given_on_the_same_terms() {
    let w = installing("https://qd.example", true);
    let issued = w.issue().await;
    let s = &w.service;
    for _ in 0..2 {
        let entry = s.role_file(&issued.token, Worker, "entry.sh").await;
        assert_eq!(entry.unwrap().sha256, "e".repeat(64));
    }
    let none = s.role_file(&issued.token, Worker, "other.sh").await;
    assert_eq!(none, Err(InstallFileError::NotFound));
    let refused = Err(InstallFileError::Refused);
    let other = s
        .role_file(&issued.token, TrinoCoordinator, "entry.sh")
        .await;
    assert_eq!(other, refused);
    assert_eq!(s.role_file("qdj_short", Worker, "entry.sh").await, refused);
    let from = origin();
    s.join(&issued.token, "worker-7", "csr", &from)
        .await
        .unwrap();
    let used = s.role_file(&issued.token, Worker, "entry.sh").await;
    assert_eq!(used, refused);
    let bare = world();
    let bare = bare.service.role_file(&issued.token, Worker, "entry.sh");
    assert_eq!(bare.await, Err(InstallFileError::Unavailable));
}

#[tokio::test]
async fn the_script_and_the_ca_are_the_files_with_their_hashes() {
    let w = installing("https://qd.example", true);
    assert_eq!(w.service.join_script().unwrap().sha256, "5".repeat(64));
    assert_eq!(w.service.install_ca().unwrap().body, "ca");
    let without_ca = installing("https://qd.example", false);
    assert_eq!(without_ca.service.install_ca(), None);
    assert_eq!(world().service.join_script(), None);
}

#[tokio::test]
async fn the_command_holds_the_hashes_the_routes_serve() {
    let w = installing("https://qd.example/", true);
    let issued = w.issue().await;
    let command = w.service.join_command(&issued, &none()).unwrap();
    assert_eq!(command.script_url, "https://qd.example/api/install/join.sh");
    assert_eq!(command.script_sha256, "5".repeat(64));
    assert_eq!(command.ca_sha256, "a".repeat(64));
    let expected = format!(
        "curl -fsS --proto '=https' -o qd-join.sh 'https://qd.example/api/install/join.sh' \\\n\
         && echo '{script}  qd-join.sh' | sha256sum -c - \\\n\
         && sh qd-join.sh --url 'https://qd.example' --role worker --pool 'default' \\\n\
         \x20    --ca-sha256 '{ca}' --token '{token}'",
        script = "5".repeat(64),
        ca = "a".repeat(64),
        token = issued.token,
    );
    assert_eq!(command.command, expected);
}

/// Where the machine's Alloy pushes its metrics (0071 point 14) is in the
/// command when the installation names it as quotable HTTPS, and only then.
#[tokio::test]
async fn the_command_names_where_the_metrics_go_when_known() {
    let url = "https://qd-app:9009/api/v1/push";
    let mut w = installing("https://qd.example", true);
    w.service = w.service.clone().with_metrics_push_url(Some(url));
    let command = w.service.join_command(&w.issue().await, &none()).unwrap();
    let expected =
        format!(" --pool 'default' \\\n     --metrics-url '{url}' \\\n     --ca-sha256 ");
    assert!(command.command.contains(&expected), "{}", command.command);
    for bad in ["http://qd-app:9009/api/v1/push", "https://x'y/push", ""] {
        let mut w = installing("https://qd.example", true);
        w.service = w.service.clone().with_metrics_push_url(Some(bad));
        let command = w.service.join_command(&w.issue().await, &none()).unwrap();
        assert!(!command.command.contains("--metrics-url"), "{bad}");
    }
}

#[tokio::test]
async fn no_command_without_https_a_ca_or_a_compose_file() {
    let issued = |w: &World| {
        let s = w.service.clone();
        async move { s.issue_join_token(Worker, None, &admin(), &origin()).await }
    };
    for url in [
        "http://qd.example",
        "https://",
        "https://qd.example/a b",
        "https://x'y",
    ] {
        let w = installing(url, true);
        let token = issued(&w).await.unwrap();
        assert_eq!(w.service.join_command(&token, &none()), None, "{url}");
    }
    let w = installing("https://qd.example", false);
    assert_eq!(
        w.service.join_command(&issued(&w).await.unwrap(), &none()),
        None
    );
    let w = installing("https://qd.example", true);
    let s = &w.service;
    let (by, from) = (admin(), origin());
    let trino = s.issue_join_token(MachineRole::TrinoWorker, None, &by, &from);
    assert_eq!(s.join_command(&trino.await.unwrap(), &none()), None);
    // Without the pool, no --pool.
    let command = s.join_command(&issued(&w).await.unwrap(), &none()).unwrap();
    assert!(!command.command.contains("--pool"));
}

fn none() -> Drives {
    Drives::default()
}
