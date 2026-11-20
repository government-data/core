//! The machines' model: transitions, roles, tokens and names.

use super::*;

#[test]
fn machine_transitions_go_forward_only() {
    use MachineState::*;
    let allowed = [
        (Pending, Approved),
        (Approved, Draining),
        (Pending, Retired),
        (Approved, Retired),
        (Draining, Retired),
    ];
    for from in MachineState::ALL {
        for to in MachineState::ALL {
            assert_eq!(
                from.may_become(to),
                allowed.contains(&(from, to)),
                "{from:?} → {to:?}"
            );
        }
    }
}

#[test]
fn machine_roles_and_states_round_trip() {
    for role in MachineRole::ALL {
        assert_eq!(MachineRole::parse(role.as_str()), Some(role));
        assert!(is_label(role.as_str()));
    }
    for state in MachineState::ALL {
        assert_eq!(MachineState::parse(state.as_str()), Some(state));
    }
    assert_eq!(MachineRole::parse("admin"), None);
    assert_eq!(MachineRole::parse("app"), Some(MachineRole::App));
    let joining: Vec<_> = MachineRole::ALL.into_iter().filter(|r| r.joins()).collect();
    assert_eq!(joining.len(), 5, "every role but app");
    assert!(!joining.contains(&MachineRole::App));
    for role in MachineRole::ALL {
        let until_replaced = matches!(role, MachineRole::TrinoCoordinator | MachineRole::App);
        let term = role.certificate_term();
        assert_eq!(term == CertificateTerm::UntilReplaced, until_replaced);
    }
    assert_eq!(MachineState::parse("Pending"), None);
    let working: Vec<_> = MachineState::ALL
        .into_iter()
        .filter(|s| s.gets_tokens())
        .collect();
    assert_eq!(working, [MachineState::Approved, MachineState::Draining]);
}

#[test]
fn machine_join_tokens_have_one_form() {
    let good = format!("qdj_{}", "A-_b".repeat(10) + "xyz");
    assert!(is_join_token(&good));
    assert!(!is_join_token(&good[..good.len() - 1]));
    assert!(!is_join_token(&good.replacen("qdj_", "qdx_", 1)));
    assert!(!is_join_token(&good.replacen('A', "+", 1)));
    assert!(!is_join_token(&good.replacen('A', "é", 1)));
    assert!(!is_join_token(""));
}

#[test]
fn machine_names_are_labels() {
    assert!(is_label("worker-01"));
    assert!(is_label(&"a".repeat(63)));
    assert!(!is_label(&"a".repeat(64)));
    assert!(!is_label(""));
    assert!(!is_label("Worker"));
    assert!(!is_label("w_1"));
    assert!(!is_label("w.1"));
}
