use std::time::{Duration, SystemTime, UNIX_EPOCH};

use prometheus_client::registry::Registry;

use super::NodeMetrics;
use crate::cert::tests::TestCa;
use crate::ports::{Renewal, Renewals};
use crate::store::FileStore;
use crate::testing::TempDir;

fn samples(registry: &Registry) -> Vec<logging::metrics::Sample> {
    let mut text = String::new();
    prometheus_client::encoding::text::encode(&mut text, registry).unwrap();
    logging::metrics::samples(&text).unwrap()
}

#[test]
fn every_label_is_a_result_of_the_fixed_list() {
    let mut registry = Registry::default();
    let metrics = NodeMetrics::register(&mut registry);
    metrics.renewed(Renewal::Ok);
    metrics.renewed(Renewal::Failed);
    metrics.renewed(Renewal::Failed);
    let results: Vec<_> = Renewal::ALL.iter().map(|r| r.as_str()).collect();
    let all = samples(&registry);
    for s in &all {
        match s.name.as_str() {
            "qd_node_renewals_total" => {
                assert_eq!(s.labels.len(), 1, "{s:?}");
                assert!(results.contains(&s.label("result").unwrap()), "{s:?}");
            }
            "qd_node_certificate_expiry_timestamp_seconds" => assert!(s.labels.is_empty()),
            other => panic!("unexpected metric {other}"),
        }
    }
    let count = |result: &str| {
        all.iter()
            .find(|s| s.label("result") == Some(result))
            .map(|s| s.value)
    };
    assert_eq!(
        (count("ok"), count("failed"), count("refused")),
        (Some(1.0), Some(2.0), Some(0.0))
    );
}

#[test]
fn the_expiry_is_the_certificate_in_use() {
    let tmp = TempDir::new();
    let store = FileStore::open(tmp.path()).unwrap();
    let mut registry = Registry::default();
    let metrics = NodeMetrics::register(&mut registry);
    // No certificate yet: nothing read, 0 stays.
    metrics.read_expiry(&store);
    let expiry = |registry: &Registry| {
        samples(registry)
            .into_iter()
            .find(|s| s.name == "qd_node_certificate_expiry_timestamp_seconds")
            .unwrap()
            .value
    };
    assert!(expiry(&registry).abs() < f64::EPSILON);
    let from = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    let lifetime = Duration::from_secs(24 * 3600);
    store
        .store_next(&TestCa::new().credentials(from, lifetime))
        .unwrap();
    assert!(store.promote().unwrap());
    metrics.read_expiry(&store);
    let ends = (from + lifetime)
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    assert!((expiry(&registry) - ends.as_secs_f64()).abs() < 1.0);
}
