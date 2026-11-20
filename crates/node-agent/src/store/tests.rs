use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, SystemTime};

use super::*;
use crate::cert::tests::TestCa;
use crate::testing::TempDir;

const DAY: Duration = Duration::from_secs(24 * 3600);

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn the_directory_is_0700_and_empty_at_first() {
    let tmp = TempDir::new();
    let dir = tmp.path().join("agent");
    let store = FileStore::open(&dir).unwrap();
    assert_eq!(mode(&dir), 0o700);
    assert!(store.current().unwrap().is_none());
    assert!(store.next().unwrap().is_none());
    assert!(store.ca().unwrap().is_none());
    assert!(!store.promote().unwrap(), "nothing to promote");
    assert!(!store.refused());
}

#[test]
fn a_promoted_renewal_is_written_whole_with_its_modes() {
    let tmp = TempDir::new();
    let store = FileStore::open(tmp.path()).unwrap();
    let ca = TestCa::new();
    store.write_ca(&ca.cert_pem).unwrap();
    let creds = ca.credentials(SystemTime::now(), DAY);
    store.store_next(&creds).unwrap();
    assert_eq!(mode(&store.path(NEXT)), 0o600);
    assert!(store.current().unwrap().is_none(), "not in use yet");
    assert!(store.promote().unwrap());

    assert_eq!(store.current().unwrap(), Some(creds.clone()));
    assert!(store.next().unwrap().is_none());
    let key = fs::read_to_string(store.path(MACHINE_KEY)).unwrap();
    let cert = fs::read_to_string(store.path(MACHINE_CERT)).unwrap();
    assert_eq!((key, cert), (creds.key_pem.clone(), creds.cert_pem.clone()));
    assert_eq!(store.ca().unwrap(), Some(ca.cert_pem.clone()));
    for (file, expected) in [
        (MACHINE_BUNDLE, 0o600),
        (MACHINE_KEY, 0o600),
        (MACHINE_CERT, 0o644),
        (CA, 0o644),
    ] {
        assert_eq!(mode(&store.path(file)), expected, "{file}");
    }
    let expected = [CA, MACHINE_CERT, MACHINE_KEY, MACHINE_BUNDLE];
    let mut expected: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
    expected.sort();
    assert_eq!(names(tmp.path()), expected, "nothing left aside");
}

#[test]
fn a_new_next_replaces_the_one_waiting() {
    let tmp = TempDir::new();
    let store = FileStore::open(tmp.path()).unwrap();
    let ca = TestCa::new();
    let (a, b) = (
        ca.credentials(SystemTime::now(), DAY),
        ca.credentials(SystemTime::now(), DAY),
    );
    store.store_next(&a).unwrap();
    store.store_next(&b).unwrap();
    assert_eq!(store.next().unwrap(), Some(b));
}

#[test]
fn a_promotion_cut_short_leaves_next_and_finishes_when_done_again() {
    let tmp = TempDir::new();
    let store = FileStore::open(tmp.path()).unwrap();
    let ca = TestCa::new();
    let old = ca.credentials(SystemTime::now(), DAY);
    store.store_next(&old).unwrap();
    store.promote().unwrap();
    let new = ca.credentials(SystemTime::now(), DAY);
    store.store_next(&new).unwrap();

    // The last rename fails, as a crash there would leave it.
    fs::remove_file(store.path(MACHINE_CERT)).unwrap();
    fs::create_dir(store.path(MACHINE_CERT)).unwrap();
    assert!(store.promote().is_err());
    assert_eq!(
        store.current().unwrap(),
        Some(new.clone()),
        "the bundle first"
    );
    let key = fs::read_to_string(store.path(MACHINE_KEY)).unwrap();
    assert_eq!(key, new.key_pem, "then the key");
    assert_eq!(store.next().unwrap(), Some(new.clone()), "next is kept");

    fs::remove_dir(store.path(MACHINE_CERT)).unwrap();
    assert!(store.promote().unwrap());
    let cert = fs::read_to_string(store.path(MACHINE_CERT)).unwrap();
    assert_eq!(cert, new.cert_pem);
    assert!(store.next().unwrap().is_none());
}

#[test]
fn a_damaged_bundle_is_an_error_not_a_missing_one() {
    let tmp = TempDir::new();
    let store = FileStore::open(tmp.path()).unwrap();
    fs::write(store.path(MACHINE_BUNDLE), "garbage").unwrap();
    assert!(store.current().is_err());
}

#[test]
fn refused_is_remembered() {
    let tmp = TempDir::new();
    let store = FileStore::open(tmp.path()).unwrap();
    store.mark_refused().unwrap();
    assert!(FileStore::open(tmp.path()).unwrap().refused());
}
