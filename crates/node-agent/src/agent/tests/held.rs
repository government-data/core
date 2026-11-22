use super::*;

const HELD: Mode = Mode::Held {
    after: Duration::from_secs(20 * 3600),
    ask_every: Duration::from_secs(300),
};

#[tokio::test]
async fn held_renews_into_next_and_takes_it_only_when_the_service_is_ready() {
    // A year, as a role that takes its certificate only at a restart gets.
    let w = World::new(YEAR);
    let start = w.clock.now();
    let mut agent = w.agent(HELD);
    assert_eq!(
        agent.step().await.unwrap(),
        Step::Join,
        "the first is taken"
    );
    let first = w.current();
    until(&mut agent, Step::Renew).await;
    let renewed = w.store().next().unwrap().unwrap();
    until(&mut agent, Step::Renew).await;
    let latest = w.store().next().unwrap().unwrap();
    assert_ne!(latest, renewed, "only the latest next is kept");
    assert_eq!(w.current(), first, "not taken");
    assert_eq!(
        w.api.presented(),
        [first.cert_pem.clone(), first.cert_pem.clone()]
    );

    // After 20 hours the service is asked; not ready, then again in 5 minutes.
    w.commands.answering(&[false, true]);
    until(&mut agent, Step::AskReady).await;
    assert_eq!(w.clock.now().duration_since(start).unwrap(), H * 20);
    assert_eq!(w.current(), first);
    assert_eq!(
        agent.step().await.unwrap(),
        Step::Wait(Duration::from_secs(300))
    );
    assert_eq!(agent.step().await.unwrap(), Step::AskReady);
    assert_eq!(w.current(), latest);
    assert!(w.store().next().unwrap().is_none());
    assert_eq!(w.ran(), ["reload", "ready?", "ready?", "reload"]);
}

#[tokio::test]
async fn held_keeps_a_next_found_at_start_until_asked() {
    let w = World::new(YEAR);
    let mut agent = w.agent(HELD);
    agent.step().await.unwrap();
    let first = w.current();
    let next = w.api.ca.credentials(w.clock.now(), YEAR);
    w.store().store_next(&next).unwrap();
    let mut agent = w.agent(HELD);
    assert!(matches!(agent.step().await.unwrap(), Step::Wait(_)));
    assert_eq!(w.current(), first);
    assert_eq!(w.store().next().unwrap(), Some(next));
}

#[tokio::test]
async fn held_does_not_wait_for_the_service_once_its_certificate_ended() {
    let w = World::new(H * 18);
    let mut agent = w.agent(HELD);
    agent.step().await.unwrap();
    until(&mut agent, Step::Renew).await;
    let next = w.store().next().unwrap().unwrap();
    until(&mut agent, Step::Renew).await;
    let latest = w.store().next().unwrap().unwrap();
    assert_ne!(next, latest);
    // The first ends at 18 hours, before the asking begins.
    until(&mut agent, Step::Promote).await;
    assert_eq!(w.current(), latest);
    assert!(!w.ran().contains(&"ready?".to_string()));
}

#[tokio::test]
async fn a_readiness_check_that_cannot_run_is_asked_again() {
    let w = World::new(YEAR);
    let mut settings = w.settings(HELD);
    settings.held_ready = Some(vec!["/nonexistent/qd-ready".into()]);
    settings.after_renew = Some(vec!["/nonexistent/qd-hook".into()]);
    let store = FileStore::open(w.dir()).unwrap();
    let processes = crate::ports::Processes::default();
    let mut agent =
        Agent::new(settings, store, w.api.clone(), w.clock.clone(), processes).with_jitter(|| 0.0);
    agent.start().unwrap();
    // The hook does not exist either: logged, the files are in place.
    assert_eq!(agent.step().await.unwrap(), Step::Join);
    for _ in 0..100 {
        if agent.step().await.unwrap() == Step::AskReady {
            break;
        }
    }
    let wait = agent.step().await.unwrap();
    assert!(matches!(wait, Step::Wait(_) | Step::Renew), "{wait:?}");
    assert!(w.store().next().unwrap().is_some(), "still held");
}
