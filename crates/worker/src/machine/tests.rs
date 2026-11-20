use super::{name_of, MachineFiles, MachineHttp};
use crate::testing::{write_machine, TempDir, TestCa};

#[test]
fn the_machines_name_is_its_certificates_cn() {
    let (ca, dir) = (TestCa::new(), TempDir::new());
    let files = MachineFiles::new(dir.path());
    assert!(!files.present(), "nothing until the agent joins");
    write_machine(dir.path(), &ca.pem, &ca.machine("worker-01"));
    assert!(files.present());
    assert_eq!(files.machine_name().unwrap(), "worker-01");
    // The bundle's first block is the certificate; the CA's has another CN.
    assert_eq!(name_of(ca.pem.as_bytes()).unwrap(), "qd test CA");
}

#[test]
fn a_file_that_is_no_certificate_gives_no_name() {
    for garbage in [
        "",
        "not a certificate",
        "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n",
    ] {
        assert!(name_of(garbage.as_bytes()).is_err(), "{garbage}");
    }
}

#[test]
fn a_client_is_built_from_the_agents_files_and_refuses_bad_ones() {
    let (ca, dir) = (TestCa::new(), TempDir::new());
    let http = MachineHttp::new(MachineFiles::new(dir.path()));
    let missing = http.client().unwrap_err().to_string();
    assert!(missing.contains("machine.pem"), "{missing}");
    let leaf = ca.machine("worker-01");
    write_machine(dir.path(), &ca.pem, &leaf);
    http.client().unwrap();
    // A key that is not there: the client cannot be built, and the error
    // holds nothing of the files.
    std::fs::write(dir.path().join("machine.pem"), &leaf.cert_pem).unwrap();
    let bad = http.client().unwrap_err().to_string();
    assert!(!bad.contains("BEGIN"), "{bad}");
}
