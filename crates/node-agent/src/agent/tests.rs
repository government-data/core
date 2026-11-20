use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;

use super::*;
use crate::api::Issued;
use crate::cert::tests::TestCa;
use crate::cert::{fingerprint, Credentials};
use crate::config::JoinToken;
use crate::plan::{self, Mode};
use crate::ports::{ManualClock, ScriptedCommands};
use crate::store::{self, FileStore};
use crate::testing::TempDir;

const H: Duration = Duration::from_secs(3600);
const YEAR: Duration = Duration::from_secs(365 * 24 * 3600);
const TOKEN: &str = "qdj_test-token";

/// What the fake API does with the next call.
enum Plan {
    Issue,
    /// Issues, and the answer is lost on the way back.
    Lose,
    /// Answers with a certificate of another CA.
    OtherCa,
    Fail(CallError),
}

#[derive(Default)]
struct Calls {
    plans: VecDeque<Plan>,
    joins: Vec<(String, String)>,
    /// The certificate presented by each renewal.
    presented: Vec<String>,
}

/// The API's two routes over a CA of the test.
#[derive(Clone)]
struct FakeApi {
    ca: Arc<TestCa>,
    clock: ManualClock,
    lifetime: Duration,
    calls: Arc<Mutex<Calls>>,
}

impl FakeApi {
    fn plan(&self, plans: impl IntoIterator<Item = Plan>) {
        self.calls.lock().unwrap().plans.extend(plans);
    }

    fn joins(&self) -> Vec<(String, String)> {
        self.calls.lock().unwrap().joins.clone()
    }

    fn presented(&self) -> Vec<String> {
        self.calls.lock().unwrap().presented.clone()
    }

    fn answer(&self, csr: &str) -> Result<Issued, CallError> {
        let plan = self.calls.lock().unwrap().plans.pop_front();
        let plan = plan.unwrap_or(Plan::Issue);
        let other;
        let ca = match &plan {
            Plan::Fail(e) => return Err(e.clone()),
            Plan::OtherCa => {
                other = TestCa::new();
                &other
            }
            Plan::Issue | Plan::Lose => &*self.ca,
        };
        let certificate = ca.sign_csr(csr, self.clock.now(), self.lifetime);
        if matches!(plan, Plan::Lose) {
            return Err(CallError::Network("connection reset".into()));
        }
        Ok(Issued {
            certificate,
            ca: ca.cert_pem.clone(),
            serial: "0a".into(),
            not_after: 0,
            state: "pending".into(),
        })
    }
}

#[async_trait]
impl MachineApi for FakeApi {
    async fn join(&self, token: &str, name: &str, csr: &str) -> Result<Issued, CallError> {
        let entry = (token.to_string(), name.to_string());
        self.calls.lock().unwrap().joins.push(entry);
        self.answer(csr)
    }

    async fn renew(&self, identity: &Credentials, _: &str, csr: &str) -> Result<Issued, CallError> {
        let presented = identity.cert_pem.clone();
        self.calls.lock().unwrap().presented.push(presented);
        self.answer(csr)
    }
}

struct World {
    tmp: TempDir,
    api: FakeApi,
    clock: ManualClock,
    commands: ScriptedCommands,
}

type TestAgent = Agent<FakeApi, ManualClock, ScriptedCommands>;

impl World {
    fn new(lifetime: Duration) -> Self {
        let tmp = TempDir::new();
        std::fs::write(tmp.path().join("token"), format!("{TOKEN}\n")).unwrap();
        // Whole seconds, as a certificate's dates are.
        let since = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH);
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(since.unwrap().as_secs());
        let clock = ManualClock::at(now);
        let api = FakeApi {
            ca: Arc::new(TestCa::new()),
            clock: clock.clone(),
            lifetime,
            calls: Arc::default(),
        };
        let commands = ScriptedCommands::default();
        Self {
            tmp,
            api,
            clock,
            commands,
        }
    }

    fn settings(&self, mode: Mode) -> Settings {
        let words = |w: &str| Some(vec![w.to_string()]);
        Settings {
            dir: self.dir().to_path_buf(),
            public_url: Some("https://calidad.example/api".into()),
            internal_url: "https://internal.example:9443/api".into(),
            name: Some("node-01".into()),
            token: Some(JoinToken::File(self.token_file())),
            ca_sha256: Some(fingerprint(&self.api.ca.cert_pem).unwrap()),
            ca_file: None,
            renew_every: H * 8,
            after_renew: words("reload"),
            mode,
            held_ready: words("ready?"),
        }
    }

    fn agent(&self, mode: Mode) -> TestAgent {
        let store = FileStore::open(self.dir()).unwrap();
        let (api, clock) = (self.api.clone(), self.clock.clone());
        let agent = Agent::new(
            self.settings(mode),
            store,
            api,
            clock,
            self.commands.clone(),
        );
        let mut agent = agent.with_jitter(|| 0.0);
        agent.start().unwrap();
        agent
    }

    fn dir(&self) -> &Path {
        self.tmp.path()
    }

    fn token_file(&self) -> std::path::PathBuf {
        self.tmp.path().join("token")
    }

    fn store(&self) -> FileStore {
        FileStore::open(self.dir()).unwrap()
    }

    fn current(&self) -> Credentials {
        self.store().current().unwrap().unwrap()
    }

    fn ran(&self) -> Vec<String> {
        self.commands
            .ran()
            .into_iter()
            .map(|c| c.join(" "))
            .collect()
    }
}

/// Steps until one is `wanted`; the steps before it only wait.
async fn until(agent: &mut TestAgent, wanted: Step) {
    for _ in 0..100 {
        let step = agent.step().await.unwrap();
        if step == wanted {
            return;
        }
        assert!(matches!(step, Step::Wait(_)), "{step:?} before {wanted:?}");
    }
    panic!("never {wanted:?}");
}

mod held;
mod join;
mod renew;
