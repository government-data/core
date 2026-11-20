use super::*;
use crate::ports::{RecordedRenewals, Renewal};

/// A machine that joined, with the agent that joined it.
async fn joined(w: &World, mode: Mode) -> TestAgent {
    let mut agent = w.agent(mode);
    assert_eq!(agent.step().await.unwrap(), Step::Join);
    agent
}

#[tokio::test]
async fn rolling_renews_with_the_old_certificate_and_takes_the_new_at_once() {
    let w = World::new(H * 24);
    let start = w.clock.now();
    let mut agent = joined(&w, Mode::Rolling).await;
    let old = w.current();
    until(&mut agent, Step::Renew).await;
    assert_eq!(w.clock.now().duration_since(start).unwrap(), H * 8);
    assert_eq!(w.api.presented(), std::slice::from_ref(&old.cert_pem));
    let next = w.store().next().unwrap().unwrap();
    assert_eq!(w.current(), old, "next first");
    assert_eq!(agent.step().await.unwrap(), Step::Promote);
    assert_eq!(w.current(), next);
    assert_ne!(next.key_pem, old.key_pem, "a new key");
    assert_eq!(w.ran(), ["reload", "reload"]);
    until(&mut agent, Step::Renew).await;
    assert_eq!(w.api.presented()[1], next.cert_pem, "the new one presented");
}

#[tokio::test]
async fn a_lost_answer_keeps_the_old_certificate_and_renews_again() {
    let w = World::new(H * 24);
    let mut agent = joined(&w, Mode::Rolling).await;
    let old = w.current();
    w.api.plan([Plan::Lose]);
    until(&mut agent, Step::Renew).await;
    assert_eq!(w.current(), old);
    assert!(w.store().next().unwrap().is_none());
    let wait = agent.step().await.unwrap();
    assert_eq!(wait, Step::Wait(Duration::from_secs(5)));
    assert_eq!(agent.step().await.unwrap(), Step::Renew);
    assert_eq!(w.api.presented(), [old.cert_pem.clone(), old.cert_pem]);
    assert_eq!(agent.step().await.unwrap(), Step::Promote);
}

#[tokio::test]
async fn too_many_attempts_wait_as_long_as_the_api_asks() {
    let w = World::new(H * 24);
    let mut agent = joined(&w, Mode::Rolling).await;
    let after = Duration::from_secs(120);
    w.api.plan([Plan::Fail(CallError::TooMany(after))]);
    until(&mut agent, Step::Renew).await;
    assert_eq!(agent.step().await.unwrap(), Step::Wait(after));
}

#[tokio::test]
async fn a_refused_renewal_stops_and_a_restart_asks_nothing() {
    let w = World::new(H * 24);
    let mut agent = joined(&w, Mode::Rolling).await;
    w.api.plan([Plan::Fail(CallError::Refused)]);
    for _ in 0..100 {
        match agent.step().await {
            Ok(Step::Wait(_)) => continue,
            Err(Fatal::Refused) => break,
            other => panic!("{other:?}"),
        }
    }
    assert!(w.dir().join(store::REFUSED).exists());
    let store = FileStore::open(w.dir()).unwrap();
    let again = Agent::new(
        w.settings(Mode::Rolling),
        store,
        w.api.clone(),
        w.clock.clone(),
        w.commands.clone(),
    );
    assert!(matches!(again.run().await, Fatal::Refused));
    assert_eq!(w.api.presented().len(), 1, "asked once");
}

#[tokio::test]
async fn a_renewal_with_another_ca_or_a_bad_request_stops() {
    let w = World::new(H * 24);
    let mut agent = joined(&w, Mode::Rolling).await;
    w.api.plan([Plan::OtherCa]);
    let stopped = loop {
        match agent.step().await {
            Ok(Step::Wait(_)) => continue,
            other => break other,
        }
    };
    assert!(matches!(stopped, Err(Fatal::CaChanged)), "{stopped:?}");
    assert!(w.store().next().unwrap().is_none());
    w.api.plan([Plan::Fail(CallError::Rejected(422))]);
    let stopped = agent.step().await;
    assert!(matches!(stopped, Err(Fatal::Rejected(_))), "{stopped:?}");
}

#[tokio::test]
async fn a_next_left_by_a_crash_is_promoted_at_start() {
    let w = World::new(H * 24);
    drop(joined(&w, Mode::Rolling).await);
    let next = w.api.ca.credentials(w.clock.now(), H * 24);
    w.store().store_next(&next).unwrap();
    let mut agent = w.agent(Mode::Rolling);
    assert_eq!(agent.step().await.unwrap(), Step::Promote);
    assert_eq!(w.current(), next);
}

#[tokio::test]
async fn at_start_a_certificate_with_under_two_thirds_left_is_renewed_at_once() {
    let w = World::new(H * 24);
    drop(joined(&w, Mode::Rolling).await);
    w.clock.advance(H * 9);
    let mut agent = w.agent(Mode::Rolling);
    assert_eq!(agent.step().await.unwrap(), Step::Renew);
}

#[tokio::test]
async fn an_ended_certificate_stops_the_agent() {
    let w = World::new(H);
    drop(joined(&w, Mode::Rolling).await);
    w.api.plan([Plan::Fail(CallError::Network("down".into()))]);
    let mut agent = w.agent(Mode::Rolling);
    let stopped = loop {
        match agent.step().await {
            Ok(_) => continue,
            Err(e) => break e,
        }
    };
    assert!(matches!(stopped, Fatal::Ended), "{stopped:?}");
}

#[tokio::test]
async fn each_renewal_says_how_it_ended() {
    let w = World::new(H * 24);
    let recorded = RecordedRenewals::default();
    let mut agent = w
        .agent(Mode::Rolling)
        .with_renewals(Arc::new(recorded.clone()));
    assert_eq!(agent.step().await.unwrap(), Step::Join);
    assert!(recorded.all().is_empty(), "joining is not renewing");
    w.api.plan([Plan::Lose]);
    until(&mut agent, Step::Renew).await;
    assert_eq!(
        agent.step().await.unwrap(),
        Step::Wait(Duration::from_secs(5))
    );
    assert_eq!(agent.step().await.unwrap(), Step::Renew);
    w.api.plan([Plan::Fail(CallError::Refused)]);
    loop {
        match agent.step().await {
            Ok(_) => continue,
            Err(Fatal::Refused) => break,
            Err(other) => panic!("{other:?}"),
        }
    }
    use Renewal::{Failed, Ok as Done, Refused};
    assert_eq!(recorded.all(), [Failed, Done, Refused]);
}
