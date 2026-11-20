//! The node agent's lines are every service's (decision 0026, point 1):
//! flat JSON with `service` and `vm`, and a sensitive field redacted (point
//! 5). A binary of its own, as a callsite first reached without a
//! subscriber by a test on another thread would stay silent here.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use node_agent::api::CallError;
use node_agent::cert::Credentials;
use node_agent::ports::{ManualClock, ScriptedCommands};
use node_agent::restart::{self, Probe, Release, Restart, RestartError};
use node_agent::store::FileStore;

struct Back;

#[async_trait]
impl Probe for Back {
    async fn back(&self, _: Duration) -> bool {
        true
    }
}

struct NeverAsked;

#[async_trait]
impl Release for NeverAsked {
    async fn release(&self, _: &Credentials, _: &str) -> Result<bool, CallError> {
        unreachable!("no certificate in use, so no release")
    }
}

/// An empty directory of its own, removed by the test.
fn empty_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("node-agent-log-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn a_node_agent_line_has_the_flat_fields_and_a_secret_is_redacted() {
    let dir = empty_dir();
    let store = FileStore::open(&dir).unwrap();
    let mut ended = None;
    let lines = logging::json_lines(node_agent::SERVICE, || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        ended = Some(runtime.block_on(async {
            tracing::info!(join_token = "qdj_not-logged", serial = "0a", "a probe");
            let how = Restart {
                command: vec!["restart".into()],
                wait: Duration::from_secs(60),
                poll: Duration::from_secs(2),
            };
            let clock = ManualClock::at(SystemTime::UNIX_EPOCH + Duration::from_secs(1));
            let commands = ScriptedCommands::default();
            restart::restart(&store, &NeverAsked, &Back, &commands, &clock, &how).await
        }));
    });
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(
        matches!(ended, Some(Err(RestartError::Files(_)))),
        "{ended:?}"
    );

    let back: Vec<_> = lines
        .iter()
        .filter(|l| l["msg"] == "the service answers again")
        .collect();
    assert_eq!(back.len(), 1, "{lines:?}");
    // Flat: no `fields` or `span` object; the map gives its keys sorted.
    let keys: Vec<_> = back[0].keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        ["after_s", "level", "msg", "service", "target", "time", "vm"]
    );
    assert_eq!(back[0]["service"], "node-agent");
    assert_eq!(back[0]["vm"], "vm-test");
    assert_eq!(back[0]["level"], "INFO");
    assert_eq!(back[0]["target"], "node_agent::restart");
    assert_eq!(back[0]["after_s"], 0, "a number stays a number");

    let probe = lines.iter().find(|l| l["msg"] == "a probe").unwrap();
    assert_eq!(probe["join_token"], logging::REDACTED);
    assert_eq!(probe["serial"], "0a");
    assert!(!format!("{lines:?}").contains("qdj_not-logged"));
}
