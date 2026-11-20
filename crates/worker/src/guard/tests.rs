//! The guard (0072, point 14): below a limit no claim is made, the gauge
//! says why, a change is logged once; readings of this host's files.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use prometheus_client::encoding::text::encode;
use prometheus_client::registry::Registry;

use super::*;

/// Free space and memory as the test sets them.
#[derive(Default)]
struct Fake {
    free: AtomicU64,
    memory_pct: AtomicU64,
}

impl Probe for Arc<Fake> {
    fn free_bytes(&self, _: &Path) -> Result<u64, String> {
        Ok(self.free.load(Ordering::SeqCst))
    }

    fn free_memory_pct(&self) -> Result<f64, String> {
        Ok(self.memory_pct.load(Ordering::SeqCst) as f64)
    }
}

fn guarded() -> (Guard, Arc<Fake>, Registry) {
    let mut registry = Registry::default();
    let metrics = WorkerMetrics::register(&mut registry, 1);
    let fake = Arc::new(Fake::default());
    fake.free.store(20_000_000_000, Ordering::SeqCst);
    fake.memory_pct.store(50, Ordering::SeqCst);
    let paths = vec![PathBuf::from("/")];
    let guard = Guard::new(
        paths,
        DEFAULT_MIN_FREE_BYTES,
        DEFAULT_MIN_FREE_MEMORY_PCT,
        Box::new(fake.clone()),
        metrics,
    );
    (guard, fake, registry)
}

/// The pause series, sorted: the registry keeps a family's series in a hash
/// map, so their order in the text changes from run to run.
fn paused(registry: &Registry) -> Vec<String> {
    let mut text = String::new();
    encode(&mut text, registry).unwrap();
    let mut lines: Vec<String> = text
        .lines()
        .filter(|l| l.starts_with("qd_worker_claim_paused{"))
        .map(str::to_string)
        .collect();
    lines.sort();
    lines
}

#[tokio::test]
async fn with_free_space_below_the_limit_no_claim_is_made() {
    let (guard, fake, registry) = guarded();
    let claimed = AtomicBool::new(false);
    let claim = || async { claimed.store(true, Ordering::SeqCst) };
    assert_eq!(guard.claim(claim).await, Some(()));
    assert!(
        claimed.swap(false, Ordering::SeqCst),
        "under no limit: claimed"
    );
    assert_eq!(
        paused(&registry),
        [
            r#"qd_worker_claim_paused{reason="disk"} 0"#,
            r#"qd_worker_claim_paused{reason="memory"} 0"#
        ]
    );

    fake.free.store(4_999_999_999, Ordering::SeqCst);
    let claim = || async { claimed.store(true, Ordering::SeqCst) };
    assert_eq!(guard.claim(claim).await, None);
    assert!(
        !claimed.load(Ordering::SeqCst),
        "at the disk's limit: no claim"
    );
    assert!(paused(&registry).contains(&r#"qd_worker_claim_paused{reason="disk"} 1"#.to_string()));

    fake.free.store(DEFAULT_MIN_FREE_BYTES, Ordering::SeqCst);
    fake.memory_pct.store(9, Ordering::SeqCst);
    let claim = || async { claimed.store(true, Ordering::SeqCst) };
    assert_eq!(guard.claim(claim).await, None);
    assert!(
        !claimed.load(Ordering::SeqCst),
        "at the memory's limit: no claim"
    );
    assert_eq!(
        paused(&registry),
        [
            r#"qd_worker_claim_paused{reason="disk"} 0"#,
            r#"qd_worker_claim_paused{reason="memory"} 1"#
        ]
    );

    fake.memory_pct.store(10, Ordering::SeqCst);
    let claim = || async { claimed.store(true, Ordering::SeqCst) };
    assert_eq!(guard.claim(claim).await, Some(()));
    assert!(
        claimed.load(Ordering::SeqCst),
        "back under its limits: claimed"
    );
    assert!(paused(&registry).iter().all(|l| l.ends_with(" 0")));
}

#[test]
fn memory_is_read_from_meminfo_and_the_cgroup() {
    let meminfo = "MemTotal:       16000000 kB\nMemFree:  1 kB\nMemAvailable:    4000000 kB\n";
    assert_eq!(host_free_pct(meminfo), Some(25.0));
    assert_eq!(host_free_pct("MemTotal: 10 kB\n"), None);
    assert_eq!(cgroup_free_pct("max\n", "100\n"), None, "no limit");
    assert_eq!(cgroup_free_pct("1000\n", "950\n"), Some(5.0));
    assert_eq!(cgroup_free_pct("1000\n", "1200\n"), Some(0.0));
}

#[test]
fn this_hosts_root_and_memory_can_be_read() {
    let free = Host.free_bytes(Path::new("/")).unwrap();
    assert!(free > 0);
    let pct = Host.free_memory_pct().unwrap();
    assert!((0.0..=100.0).contains(&pct), "{pct}");
}
