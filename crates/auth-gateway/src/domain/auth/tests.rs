use super::*;

#[test]
fn password_policy_reports_every_unmet_requirement() {
    let policy = PasswordPolicy::recommended();
    let err = policy.validate("short").unwrap_err();
    assert!(err.unmet.contains(&PasswordRequirement::MinLength(12)));
    assert!(err.unmet.contains(&PasswordRequirement::Uppercase));
    assert!(err.unmet.contains(&PasswordRequirement::Digit));

    assert!(policy.validate("Str0ngEnoughPass").is_ok());
}

#[test]
fn symbol_requirement_is_configurable() {
    let policy = PasswordPolicy {
        require_symbol: true,
        ..PasswordPolicy::recommended()
    };
    assert!(policy
        .validate("Str0ngEnoughPass")
        .unwrap_err()
        .unmet
        .contains(&PasswordRequirement::Symbol));
    assert!(policy.validate("Str0ngEnough!Pass").is_ok());
}

#[test]
fn lockout_engages_at_threshold_and_backs_off_progressively() {
    let policy = LockoutPolicy {
        max_attempts: 3,
        base_delay: Duration::from_secs(10),
        max_delay: Duration::from_secs(80),
    };
    let now = SystemTime::UNIX_EPOCH;

    // Below the threshold: counting up, not yet locked.
    let s1 = policy.register_failure(&LockoutState::clear(), now);
    assert_eq!(s1.failed_attempts, 1);
    assert!(!policy.is_locked(&s1, now));
    let s2 = policy.register_failure(&s1, now);
    assert!(!policy.is_locked(&s2, now));

    // At the threshold: locked for base_delay.
    let s3 = policy.register_failure(&s2, now);
    assert_eq!(s3.failed_attempts, 3);
    assert!(policy.is_locked(&s3, now));
    assert_eq!(s3.locked_until, Some(now + Duration::from_secs(10)));
    assert!(!policy.is_locked(&s3, now + Duration::from_secs(10)));

    // Past the threshold: the delay doubles (20s), then clamps at max (80s).
    let s4 = policy.register_failure(&s3, now);
    assert_eq!(s4.locked_until, Some(now + Duration::from_secs(20)));
    let mut s = s4;
    for _ in 0..10 {
        s = policy.register_failure(&s, now);
    }
    assert_eq!(s.locked_until, Some(now + Duration::from_secs(80)));
}

#[test]
fn retry_after_reports_remaining_lock() {
    let policy = LockoutPolicy::recommended();
    let now = SystemTime::UNIX_EPOCH;
    let locked = LockoutState {
        failed_attempts: 5,
        locked_until: Some(now + Duration::from_secs(30)),
    };
    assert_eq!(
        policy.retry_after(&locked, now),
        Some(Duration::from_secs(30))
    );
    assert_eq!(
        policy.retry_after(&locked, now + Duration::from_secs(30)),
        None
    );
    assert_eq!(policy.retry_after(&LockoutState::clear(), now), None);
}
