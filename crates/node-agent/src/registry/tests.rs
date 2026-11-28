use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::SystemTime;

use super::*;
use crate::cert::tests::TestCa;
use crate::testing::TempDir;

const YEAR: Duration = Duration::from_secs(365 * 24 * 3600);
const PASSWORD: &str = "registry-password";

/// The core's route: answers given in turn.
#[derive(Default)]
struct Gate(Mutex<VecDeque<Result<RegistryGrant, CallError>>>);

#[async_trait]
impl RegistryGate for Gate {
    async fn registry(&self, _: &Credentials, _: &str) -> Result<RegistryGrant, CallError> {
        let next = self.0.lock().unwrap().pop_front();
        next.unwrap_or(Err(CallError::Unavailable(503)))
    }
}

/// Docker: what it was asked, the directory's mode at each call, and a
/// pull that fails when told.
#[derive(Default)]
struct FakeDocker {
    calls: Mutex<Vec<String>>,
    fail_pull: bool,
}

impl FakeDocker {
    fn seen(&self, config: &Path, call: String) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(config).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "{call}");
        }
        assert!(
            std::fs::read_dir(config).is_ok(),
            "{call}: the directory exists"
        );
        self.calls.lock().unwrap().push(call);
    }
}

#[async_trait]
impl Docker for FakeDocker {
    async fn login(&self, config: &Path, grant: &RegistryGrant) -> Result<(), String> {
        assert_eq!(grant.password, PASSWORD);
        std::fs::write(config.join("config.json"), "{\"auths\":{}}").unwrap();
        self.seen(config, format!("login {} {}", grant.username, grant.host));
        Ok(())
    }

    async fn pull(&self, config: &Path, image: &str) -> Result<(), String> {
        self.seen(config, format!("pull {image}"));
        if self.fail_pull {
            Err("manifest unknown".into())
        } else {
            Ok(())
        }
    }

    async fn up(
        &self,
        config: &Path,
        module: &str,
        dir: &Path,
        services: &[String],
    ) -> Result<(), String> {
        assert!(
            dir.join("compose.yml").is_file(),
            "its files are in place first"
        );
        self.seen(config, format!("up {module} {}", services.join(",")));
        Ok(())
    }
}

struct World {
    dir: TempDir,
    store: FileStore,
}

fn world() -> World {
    let dir = TempDir::new();
    let store = FileStore::open(dir.path()).unwrap();
    let ca = TestCa::new();
    store.write_ca(&ca.cert_pem).unwrap();
    store
        .store_next(&ca.credentials(SystemTime::now(), YEAR))
        .unwrap();
    store.promote().unwrap();
    World { dir, store }
}

fn grant() -> RegistryGrant {
    RegistryGrant {
        host: "registry.example.com".into(),
        username: "AWS".into(),
        password: PASSWORD.into(),
        expires_at: "2026-11-25T18:00:00Z".into(),
        images: vec![
            "registry.example.com/a@sha256:aa".into(),
            "registry.example.com/b@sha256:bb".into(),
        ],
        modules: vec![crate::role_files::RoleModule {
            name: "lake-rustfs".into(),
            version: "0.2.1".into(),
            services: vec!["rustfs".into(), "gate".into()],
            files: vec![role_file("compose.yml", b"services: {}\n")],
            env: Default::default(),
            secrets: Default::default(),
        }],
    }
}

fn role_file(path: &str, bytes: &[u8]) -> crate::role_files::RoleFile {
    use base64::Engine as _;
    use sha2::{Digest, Sha256};
    crate::role_files::RoleFile {
        path: path.into(),
        sha256: Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        content_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
    }
}

/// No configuration directory is left in the agent's.
fn none_left(w: &World) {
    let left: Vec<_> = std::fs::read_dir(w.dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with("docker-config"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[tokio::test]
async fn an_answer_logs_in_pulls_each_image_and_removes_the_directory() {
    let w = world();
    let gate = Gate(Mutex::new([Ok(grant())].into()));
    let docker = FakeDocker::default();
    let pulled = pull_role(&w.store, &gate, &docker).await;
    assert_eq!(pulled, Pulled::All(2));
    assert_eq!(pulled.next_in(), ASK_EVERY);
    assert_eq!(
        *docker.calls.lock().unwrap(),
        [
            "login AWS registry.example.com",
            "pull registry.example.com/a@sha256:aa",
            "pull registry.example.com/b@sha256:bb",
            "up lake-rustfs rustfs,gate",
        ]
    );
    let placed = w.dir.path().join("modules/lake-rustfs/compose.yml");
    assert_eq!(std::fs::read_to_string(placed).unwrap(), "services: {}\n");
    none_left(&w);
}

#[tokio::test]
async fn a_failed_pull_still_removes_the_directory_and_retries_soon() {
    let w = world();
    let gate = Gate(Mutex::new([Ok(grant())].into()));
    let docker = FakeDocker {
        fail_pull: true,
        ..FakeDocker::default()
    };
    let pulled = pull_role(&w.store, &gate, &docker).await;
    assert!(matches!(&pulled, Pulled::Retry(e) if e.contains("manifest unknown")));
    assert!(!format!("{pulled:?}").contains(PASSWORD));
    assert_eq!(pulled.next_in(), RETRY_AFTER);
    none_left(&w);
}

#[tokio::test]
async fn no_token_is_retried_in_ten_minutes_and_another_role_in_six_hours() {
    let w = world();
    let docker = FakeDocker::default();
    let gate = Gate(Mutex::new(
        [
            Err(CallError::Unavailable(503)),
            Err(CallError::Rejected(403)),
        ]
        .into(),
    ));
    let no_token = pull_role(&w.store, &gate, &docker).await;
    assert_eq!(no_token.next_in(), Duration::from_secs(600));
    let other_role = pull_role(&w.store, &gate, &docker).await;
    assert_eq!(other_role, Pulled::NotForRole);
    assert_eq!(other_role.next_in(), Duration::from_secs(6 * 3600));
    assert!(docker.calls.lock().unwrap().is_empty(), "nothing pulled");
}

#[tokio::test]
async fn before_joining_nothing_is_asked() {
    let dir = TempDir::new();
    let store = FileStore::open(dir.path()).unwrap();
    let pulled = pull_role(&store, &Gate::default(), &FakeDocker::default()).await;
    assert!(matches!(pulled, Pulled::Retry(_)));
}

#[test]
fn the_grants_debug_shows_no_password() {
    let shown = format!("{:?}", grant());
    assert!(!shown.contains(PASSWORD), "{shown}");
}

#[cfg(unix)]
#[tokio::test]
async fn the_cli_gives_the_password_on_its_input_only() {
    // A stand-in `docker` that writes its arguments and its input apart.
    let dir = TempDir::new();
    let script = dir.path().join("docker");
    let out = dir.path().join("out");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho \"$@\" > {0}.args\ncat > {0}.input\n",
            out.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let cli = DockerCli {
        program: script,
        timeout: Duration::from_secs(10),
    };
    let config = dir.path().join("config");
    cli.login(&config, &grant()).await.unwrap();
    let args = std::fs::read_to_string(dir.path().join("out.args")).unwrap();
    let input = std::fs::read_to_string(dir.path().join("out.input")).unwrap();
    assert!(
        args.contains("--password-stdin registry.example.com"),
        "{args}"
    );
    assert!(args.contains("--config"), "{args}");
    assert!(!args.contains(PASSWORD), "never an argument");
    assert_eq!(input, PASSWORD);
}
