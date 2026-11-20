use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::*;

const M: Duration = Duration::from_secs(60);
const H: Duration = Duration::from_secs(3600);
const DAY: Duration = Duration::from_secs(24 * 3600);
const HELD: Mode = Mode::Held {
    after: DAY,
    ask_every: Duration::from_secs(300),
};

fn t(hours: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_800_000_000) + H * u32::try_from(hours).unwrap()
}

fn valid(from: u64, hours: u64) -> Validity {
    Validity {
        not_before: t(from),
        not_after: t(from + hours),
    }
}

/// At hour `now`, a day's certificate from hour 0, the API due at `call`.
fn facts(now: u64, call: u64) -> Facts {
    Facts {
        now: t(now),
        current: Some(valid(0, 24)),
        next: None,
        call_at: t(call),
        ask_at: t(0),
    }
}

#[test]
fn a_machine_without_a_certificate_joins_when_due() {
    let f = Facts {
        current: None,
        ..facts(0, 0)
    };
    assert_eq!(decide(Mode::Rolling, &f), Step::Join);
    assert_eq!(decide(HELD, &f), Step::Join);
    let later = Facts { call_at: t(1), ..f };
    assert_eq!(decide(Mode::Rolling, &later), Step::Wait(H));
}

#[test]
fn a_join_cut_short_before_promoting_is_promoted() {
    let f = Facts {
        current: None,
        next: Some(valid(0, 24)),
        ..facts(0, 5)
    };
    assert_eq!(decide(Mode::Rolling, &f), Step::Promote);
    assert_eq!(decide(HELD, &f), Step::Promote, "nothing to hold");
}

#[test]
fn rolling_renews_when_due_and_takes_the_next_at_once() {
    assert_eq!(decide(Mode::Rolling, &facts(8, 8)), Step::Renew);
    assert_eq!(
        decide(Mode::Rolling, &facts(2, 8)),
        Step::Wait(LONGEST_WAIT)
    );
    let soon = Facts {
        now: t(8) - M * 20,
        ..facts(0, 8)
    };
    assert_eq!(decide(Mode::Rolling, &soon), Step::Wait(M * 20));
    let pending = Facts {
        next: Some(valid(8, 24)),
        ..facts(8, 16)
    };
    assert_eq!(decide(Mode::Rolling, &pending), Step::Promote);
}

#[test]
fn an_ended_certificate_takes_a_next_if_there_is_one_else_ends() {
    let ended = facts(24, 30);
    assert_eq!(decide(Mode::Rolling, &ended), Step::Ended);
    assert_eq!(decide(HELD, &ended), Step::Ended);
    let with_next = Facts {
        next: Some(valid(20, 24)),
        ..ended
    };
    assert_eq!(decide(HELD, &with_next), Step::Promote, "not held then");
    let next_ended = Facts {
        next: Some(valid(0, 24)),
        ..ended
    };
    assert_eq!(decide(Mode::Rolling, &next_ended), Step::Ended);
}

#[test]
fn held_keeps_renewing_into_next_and_asks_only_after_a_day() {
    let year = Validity {
        not_before: t(0),
        not_after: t(24 * 365),
    };
    let held = |now, call, ask| Facts {
        now: t(now),
        current: Some(year),
        next: Some(valid(now.saturating_sub(1), 24)),
        call_at: t(call),
        ask_at: t(ask),
    };
    assert_eq!(
        decide(HELD, &held(8, 8, 0)),
        Step::Renew,
        "the next replaced"
    );
    // Before the day: it waits for the renewal or the day, whichever first.
    let before = |now: SystemTime, call| Facts {
        now,
        ..held(23, call, 0)
    };
    assert_eq!(decide(HELD, &before(t(16) - M, 16)), Step::Wait(M));
    assert_eq!(decide(HELD, &before(t(24) - M * 2, 30)), Step::Wait(M * 2));
    assert_eq!(decide(HELD, &held(24, 30, 0)), Step::AskReady);
    // Asked and not ready: the next question at `ask_at`.
    let asked = Facts {
        ask_at: t(25) + M * 5,
        ..held(25, 30, 0)
    };
    assert_eq!(decide(HELD, &asked), Step::Wait(M * 5));
    assert_eq!(decide(HELD, &held(26, 30, 26)), Step::AskReady);
    // Without a next there is nothing to ask about: only the renewal.
    let none = Facts {
        next: None,
        now: t(30) - M * 3,
        ..held(26, 30, 26)
    };
    assert_eq!(decide(HELD, &none), Step::Wait(M * 3));
}

#[test]
fn a_wait_is_at_least_a_second_and_at_most_an_hour() {
    assert_eq!(
        decide(Mode::Rolling, &facts(0, 20)),
        Step::Wait(LONGEST_WAIT)
    );
    let f = Facts {
        now: t(1) - Duration::from_millis(10),
        ..facts(0, 1)
    };
    assert_eq!(
        decide(Mode::Rolling, &f),
        Step::Wait(Duration::from_secs(1))
    );
}

#[test]
fn the_first_renewal_is_at_once_under_two_thirds_left() {
    let cert = valid(0, 24);
    assert_eq!(first_renewal(t(1), &cert, H * 8), t(9));
    assert_eq!(first_renewal(t(8), &cert, H * 8), t(16), "two thirds left");
    assert_eq!(first_renewal(t(9), &cert, H * 8), t(9));
}

#[test]
fn the_jitter_is_ten_percent_at_most() {
    let every = H * 8;
    assert_eq!(jittered(every, 0.0), every);
    assert_eq!(jittered(every, 1.0), every.mul_f64(1.1));
    assert_eq!(jittered(every, -1.0), every.mul_f64(0.9));
    assert_eq!(jittered(every, 7.0), every.mul_f64(1.1), "clamped");
    assert_eq!(jittered(every, f64::NAN), every);
}

#[test]
fn backoff_doubles_from_five_seconds_up_to_ten_minutes() {
    let s = Duration::from_secs;
    let waits: Vec<_> = (1..=9).map(backoff).collect();
    assert_eq!(
        waits,
        [
            s(5),
            s(10),
            s(20),
            s(40),
            s(80),
            s(160),
            s(320),
            s(600),
            s(600)
        ]
    );
    assert_eq!(backoff(0), s(5));
    assert_eq!(backoff(u32::MAX), MAX_BACKOFF);
}
