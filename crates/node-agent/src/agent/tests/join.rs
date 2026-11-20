use super::*;

#[tokio::test]
async fn a_join_writes_the_files_deletes_the_token_and_runs_the_hook() {
    let w = World::new(H * 24);
    let mut agent = w.agent(Mode::Rolling);
    assert_eq!(agent.step().await.unwrap(), Step::Join);
    assert_eq!(w.api.joins(), [(TOKEN.into(), "node-01".into())]);
    assert!(w.current().validity().is_ok(), "a key and its certificate");
    assert_eq!(w.store().ca().unwrap(), Some(w.api.ca.cert_pem.clone()));
    assert!(w.store().next().unwrap().is_none());
    assert!(!w.token_file().exists(), "the token is used once");
    assert_eq!(w.ran(), ["reload"]);
    // Next: the renewal, eight hours on.
    assert_eq!(agent.step().await.unwrap(), Step::Wait(plan::LONGEST_WAIT));
}

#[tokio::test]
async fn a_join_answered_with_another_ca_writes_nothing() {
    let w = World::new(H * 24);
    w.api.plan([Plan::OtherCa]);
    let mut agent = w.agent(Mode::Rolling);
    assert!(matches!(agent.step().await, Err(Fatal::CaMismatch)));
    let names = std::fs::read_dir(w.dir()).unwrap().count();
    assert_eq!(names, 1, "only the token's file");
    assert!(w.ran().is_empty());
}

#[tokio::test]
async fn a_refused_taken_or_rejected_join_stops_and_keeps_the_token() {
    for (e, fatal) in [
        (CallError::Refused, "refused"),
        (CallError::NameTaken, "name"),
        (CallError::Rejected(422), "rejected"),
    ] {
        let w = World::new(H * 24);
        w.api.plan([Plan::Fail(e)]);
        let mut agent = w.agent(Mode::Rolling);
        let stopped = agent.step().await.unwrap_err();
        assert!(stopped.to_string().contains(fatal), "{stopped}");
        assert!(w.store().current().unwrap().is_none());
        assert!(w.token_file().exists());
    }
}

#[tokio::test]
async fn a_join_without_an_answer_is_tried_again_backing_off() {
    let w = World::new(H * 24);
    w.api.plan([
        Plan::Fail(CallError::Network("refused".into())),
        Plan::Fail(CallError::Unavailable(503)),
    ]);
    let start = w.clock.now();
    let mut agent = w.agent(Mode::Rolling);
    assert_eq!(agent.step().await.unwrap(), Step::Join);
    let five = Duration::from_secs(5);
    assert_eq!(agent.step().await.unwrap(), Step::Wait(five));
    assert_eq!(agent.step().await.unwrap(), Step::Join);
    assert_eq!(agent.step().await.unwrap(), Step::Wait(five * 2));
    assert_eq!(agent.step().await.unwrap(), Step::Join);
    assert_eq!(w.clock.now().duration_since(start).unwrap(), five * 3);
    assert_eq!(w.api.joins().len(), 3);
    assert!(w.store().current().unwrap().is_some());
}

#[tokio::test]
async fn a_join_needs_its_settings() {
    let w = World::new(H * 24);
    let mut settings = w.settings(Mode::Rolling);
    settings.ca_sha256 = None;
    let store = FileStore::open(w.dir()).unwrap();
    let commands = w.commands.clone();
    let mut agent = Agent::new(settings, store, w.api.clone(), w.clock.clone(), commands);
    agent.start().unwrap();
    let stopped = agent.step().await.unwrap_err();
    assert!(stopped.to_string().contains("QD_CA_SHA256"), "{stopped}");
    assert!(w.api.joins().is_empty(), "nothing asked");
}
