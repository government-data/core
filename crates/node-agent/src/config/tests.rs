use std::collections::HashMap;
use std::time::Duration;

use super::*;

fn settings(pairs: &[(&str, &str)]) -> Result<Settings, String> {
    let env: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    Settings::from(|name| env.get(name).cloned())
}

const BASE: [(&str, &str); 2] = [
    ("QD_AGENT_DIR", "/var/lib/qd-node"),
    ("QD_INTERNAL_URL", "https://internal.example:9443/api"),
];

#[test]
fn the_defaults_renew_every_eight_hours_rolling() {
    let s = settings(&BASE).unwrap();
    assert_eq!(s.renew_every, Duration::from_secs(8 * 3600));
    assert_eq!(s.mode, Mode::Rolling);
    assert!(s.token.is_none() && s.after_renew.is_none() && s.ca_file.is_none());
    assert!(settings(&BASE[..1])
        .unwrap_err()
        .contains("QD_INTERNAL_URL"));
    assert!(settings(&BASE[1..]).unwrap_err().contains("QD_AGENT_DIR"));
}

#[test]
fn the_token_file_wins_and_is_never_printed() {
    let mut env = BASE.to_vec();
    env.push(("QD_JOIN_TOKEN", "qdj_secret"));
    let s = settings(&env).unwrap();
    assert!(matches!(&s.token, Some(JoinToken::Value(v)) if v == "qdj_secret"));
    assert!(!format!("{s:?}").contains("qdj_secret"));
    env.push(("QD_JOIN_TOKEN_FILE", "/run/qd/token"));
    let s = settings(&env).unwrap();
    assert!(matches!(&s.token, Some(JoinToken::File(p)) if p.as_os_str() == "/run/qd/token"));
}

#[test]
fn held_needs_its_command_and_takes_its_times() {
    let mut env = BASE.to_vec();
    env.push(("QD_RENEW_MODE", "held"));
    assert!(settings(&env).unwrap_err().contains("QD_HELD_READY"));
    env.push((
        "QD_HELD_READY",
        r#"["/usr/local/bin/service-idle", "--twice"]"#,
    ));
    let s = settings(&env).unwrap();
    let five_minutes = Duration::from_secs(300);
    let day = Duration::from_secs(86_400);
    assert_eq!(
        s.mode,
        Mode::Held {
            after: day,
            ask_every: five_minutes
        }
    );
    assert_eq!(
        s.held_ready.unwrap(),
        ["/usr/local/bin/service-idle", "--twice"]
    );
    env.extend([("QD_HELD_AFTER", "2h"), ("QD_HELD_ASK_EVERY", "30s")]);
    let s = settings(&env).unwrap();
    let (after, ask_every) = (Duration::from_secs(7200), Duration::from_secs(30));
    assert_eq!(s.mode, Mode::Held { after, ask_every });
    env.push(("QD_RENEW_MODE", "sometimes"));
    assert!(settings(&env).is_err());
}

#[test]
fn durations_take_a_unit() {
    let s = Duration::from_secs;
    assert_eq!(parse_duration("90").unwrap(), s(90));
    assert_eq!(parse_duration("90s").unwrap(), s(90));
    assert_eq!(parse_duration("5m").unwrap(), s(300));
    assert_eq!(parse_duration(" 8h ").unwrap(), s(28_800));
    assert_eq!(parse_duration("1d").unwrap(), s(86_400));
    for bad in ["0", "0h", "h", "-1h", "8 hours", "1.5h", "8w", ""] {
        assert!(parse_duration(bad).is_err(), "{bad}");
    }
}

#[test]
fn commands_are_words_or_a_json_array_never_a_shell() {
    let words = parse_command("docker kill --signal HUP postgres").unwrap();
    assert_eq!(words, ["docker", "kill", "--signal", "HUP", "postgres"]);
    let json = parse_command(r#"["sh", "-c", "a b"]"#).unwrap();
    assert_eq!(json, ["sh", "-c", "a b"], "a shell only when asked for");
    assert!(parse_command("[]").is_err());
    assert!(parse_command(r#"[""]"#).is_err());
    assert!(parse_command("[1, 2]").is_err());
    let mut env = BASE.to_vec();
    env.push(("QD_AFTER_RENEW", "pg_ctl reload"));
    assert_eq!(
        settings(&env).unwrap().after_renew.unwrap(),
        ["pg_ctl", "reload"]
    );
}

#[test]
fn dolts_follower_is_on_only_with_its_restart() {
    assert!(DoltSettings::from(|_| None).unwrap().is_none(), "off");
    let env: HashMap<&str, &str> = [
        ("QD_DOLT_RESTART", "docker restart data-quality-dolt-dolt-1"),
        ("QD_DOLT_ADDR", "qd-app:3306"),
        ("QD_HELD_AFTER", "12h"),
    ]
    .into();
    let get = |env: &HashMap<&str, &str>, name: &str| env.get(name).map(|v| v.to_string());
    let s = DoltSettings::from(|n| get(&env, n)).unwrap().unwrap();
    assert_eq!(s.restart, ["docker", "restart", "data-quality-dolt-dolt-1"]);
    assert_eq!(s.addr, "qd-app:3306");
    assert_eq!(s.after, Duration::from_secs(12 * 3600));
    assert_eq!(s.every, Duration::from_secs(300));
    assert_eq!(s.wait, Duration::from_secs(120));
    let mut no_addr = env.clone();
    no_addr.remove("QD_DOLT_ADDR");
    assert!(DoltSettings::from(|n| get(&no_addr, n)).is_err());
}
