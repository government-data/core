//! The application VM's machine (0066, point 16): made `approved` with the
//! role `app` by the installer, its first certificate signed on the spot;
//! a second run makes no second machine.

use super::*;
use AuditEventType::{CertificateIssued, MachineApproved};

async fn install(w: &World, name: &str, held: Option<&str>) -> Result<Installed, InstallError> {
    w.service.install_app(name, "csr", held, &origin()).await
}

#[tokio::test]
async fn the_app_machine_is_made_approved_with_its_certificate_in_use() {
    let w = world();
    let Installed::Signed {
        machine,
        certificate,
        ca,
    } = install(&w, "qd-app", None).await.unwrap()
    else {
        panic!("signed");
    };
    assert_eq!(machine.name, "qd-app");
    assert_eq!(machine.role, MachineRole::App);
    assert_eq!(machine.state, MachineState::Approved);
    assert_eq!(machine.approved_by, None);
    assert_eq!(machine.cert_serial.as_deref(), Some("ab12"));
    assert_eq!(certificate.serial, "ab12");
    assert_eq!(certificate.pem, "cert app qd-app");
    // The role's term, a year (`CertificateTerm::UntilReplaced`).
    let year = SystemTime::UNIX_EPOCH + DAY * 365;
    assert_eq!(certificate.not_after, year);
    assert_eq!(machine.cert_not_after, Some(year));
    assert_eq!(ca, "ca");

    assert_eq!(w.kinds(), [MachineApproved, CertificateIssued]);
    for event in w.audit.0.lock().unwrap().iter() {
        assert_eq!(event.action, "install:app");
        assert_eq!(event.resource, "machine/qd-app");
        assert_eq!(event.admin_id, None);
        assert_eq!(event.service_account.as_deref(), Some("qd-app"));
    }
}

#[tokio::test]
async fn a_second_run_holding_the_certificate_does_nothing() {
    let w = world();
    let first = install(&w, "qd-app", None).await.unwrap();
    let Installed::Signed { machine, .. } = first else {
        panic!("signed");
    };
    let again = install(&w, "qd-app", Some("AB12")).await.unwrap();
    assert_eq!(again, Installed::Unchanged(machine));
    assert_eq!(w.service.list().await.unwrap().len(), 1);
    assert_eq!(w.kinds(), [MachineApproved, CertificateIssued]);
}

#[tokio::test]
async fn a_second_run_without_the_certificate_signs_again_for_the_same_machine() {
    let w = world();
    let Installed::Signed { machine: first, .. } = install(&w, "qd-app", None).await.unwrap()
    else {
        panic!("signed");
    };
    for held in [None, Some("ffff")] {
        let Installed::Signed { machine, .. } = install(&w, "qd-app", held).await.unwrap() else {
            panic!("signed again");
        };
        assert_eq!(machine.id, first.id);
        assert_eq!(machine.state, MachineState::Approved);
    }
    assert_eq!(w.service.list().await.unwrap().len(), 1);
    let issued = CertificateIssued;
    assert_eq!(w.kinds(), [MachineApproved, issued, issued, issued]);
}

#[tokio::test]
async fn a_name_of_another_role_is_refused() {
    let w = world();
    let worker = w.joined("qd-app").await;
    let before = w.kinds();
    let taken = install(&w, "qd-app", None).await;
    assert_eq!(taken, Err(InstallError::NameTaken));
    assert_eq!(w.kinds(), before);
    let machines = w.service.list().await.unwrap();
    assert_eq!(machines, [worker]);
}

#[tokio::test]
async fn no_ca_a_bad_name_or_a_bad_request_makes_no_machine() {
    let audit = Arc::new(Audit::default());
    let clock = Arc::new(FixedClock(Mutex::new(SystemTime::UNIX_EPOCH)));
    let service = MachinesService::new(
        Arc::new(Memory::default()),
        Arc::new(Mint::default()),
        audit.clone(),
        clock,
    );
    let none = service.install_app("qd-app", "csr", None, &origin()).await;
    assert_eq!(none, Err(InstallError::Unavailable));

    let w = world();
    assert_eq!(
        install(&w, "QD_APP", None).await,
        Err(InstallError::InvalidName)
    );
    let bad = w
        .service
        .install_app("qd-app", "bad", None, &origin())
        .await;
    assert_eq!(bad, Err(InstallError::InvalidRequest));
    assert!(w.service.list().await.unwrap().is_empty());
    assert!(w.kinds().is_empty());
    assert!(audit.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn the_update_agent_is_made_approved_with_the_role_updater() {
    let w = world();
    let installed = w
        .service
        .install_updater("updater", "csr", None, &origin())
        .await
        .unwrap();
    let Installed::Signed {
        machine,
        certificate,
        ..
    } = installed
    else {
        panic!("signed");
    };
    assert_eq!(
        (machine.role, machine.state),
        (MachineRole::Updater, MachineState::Approved)
    );
    assert_eq!(certificate.pem, "cert updater updater");
    let year = SystemTime::UNIX_EPOCH + DAY * 365;
    assert_eq!(certificate.not_after, year, "until replaced");
    let actions: Vec<_> = w
        .audit
        .0
        .lock()
        .unwrap()
        .iter()
        .map(|e| e.action.clone())
        .collect();
    assert_eq!(actions, ["install:updater", "install:updater"]);

    // Not as the application VM: a name is one machine of one role.
    let taken = w
        .service
        .install_app("updater", "csr", None, &origin())
        .await;
    assert_eq!(taken, Err(InstallError::NameTaken));
    let again = w
        .service
        .install_updater("updater", "csr", Some(&certificate.serial), &origin())
        .await;
    assert_eq!(again, Ok(Installed::Unchanged(machine)));
}
