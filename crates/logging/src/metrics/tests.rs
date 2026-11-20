use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::registry::Registry;

use super::process::{cpu_seconds_of, resident_bytes_of};
use super::{samples, MetricsFile};

/// A directory of its own for a test, removed when it ends.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("qd-metrics-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

type Jobs = Family<[(&'static str, &'static str); 1], Counter>;

fn registry() -> (Registry, Jobs) {
    let mut registry = Registry::default();
    let jobs = Jobs::default();
    registry.register("qd_test_jobs", "Synthetic jobs.", jobs.clone());
    (registry, jobs)
}

#[test]
fn the_file_is_written_aside_renamed_and_readable_by_everyone() {
    let dir = TempDir::new();
    let (registry, jobs) = registry();
    jobs.get_or_create(&[("kind", "evaluate")]).inc_by(3);
    let file = MetricsFile::new("api", &dir.0, registry);
    assert!(file.tick());
    let path = dir.0.join("api.prom");
    assert_eq!(file.path(), path);
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o644);
    let names: Vec<_> = std::fs::read_dir(&dir.0)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, ["api.prom"], "nothing left aside");
    let text = std::fs::read_to_string(&path).unwrap();
    let all = samples(&text).unwrap();
    let jobs = all.iter().find(|s| s.name == "qd_test_jobs_total").unwrap();
    assert_eq!(jobs.label("kind"), Some("evaluate"));
    assert!((jobs.value - 3.0).abs() < f64::EPSILON);
}

#[test]
fn the_process_series_carry_the_service_and_nothing_else() {
    let dir = TempDir::new();
    let file = MetricsFile::new("worker", &dir.0, Registry::default());
    let all = samples(&file.render()).unwrap();
    for name in ["process_cpu_seconds_total", "process_resident_memory_bytes"] {
        let s = all.iter().find(|s| s.name == name).expect(name);
        assert_eq!(s.labels, [("service".to_string(), "worker".to_string())]);
        assert!(s.value >= 0.0);
    }
}

#[test]
fn a_rewrite_replaces_the_file_whole() {
    let dir = TempDir::new();
    let (registry, jobs) = registry();
    let file = MetricsFile::new("api", &dir.0, registry);
    jobs.get_or_create(&[("kind", "map")]).inc();
    assert!(file.tick());
    jobs.get_or_create(&[("kind", "map")]).inc();
    assert!(file.tick());
    let text = std::fs::read_to_string(file.path()).unwrap();
    let map: Vec<_> = samples(&text)
        .unwrap()
        .into_iter()
        .filter(|s| s.name == "qd_test_jobs_total")
        .collect();
    assert_eq!(map.len(), 1);
    assert!((map[0].value - 2.0).abs() < f64::EPSILON);
}

#[test]
fn a_missing_directory_warns_once_and_writing_resumes_when_it_is_there() {
    let dir = TempDir::new();
    let missing = dir.0.join("not-there");
    let file = MetricsFile::new("node-agent", &missing, Registry::default());
    let lines = crate::json_lines("node-agent", || {
        assert!(!file.tick());
        assert!(!file.tick());
        assert!(!file.tick());
        std::fs::create_dir(&missing).unwrap();
        assert!(file.tick());
    });
    let levels: Vec<_> = lines.iter().map(|l| l["level"].as_str().unwrap()).collect();
    assert_eq!(levels, ["WARN", "INFO"], "{lines:?}");
    assert!(missing.join("node-agent.prom").exists());
}

#[test]
fn a_read_only_directory_is_not_a_failure_of_the_service() {
    let dir = TempDir::new();
    std::fs::set_permissions(&dir.0, std::fs::Permissions::from_mode(0o555)).unwrap();
    let file = MetricsFile::new("api", &dir.0, Registry::default());
    // Root writes anyway; anyone else is refused, and the tick says so.
    let written = file.tick();
    std::fs::set_permissions(&dir.0, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(written, dir.0.join("api.prom").exists());
}

#[test]
fn the_refresh_runs_before_each_write() {
    let dir = TempDir::new();
    let (registry, jobs) = registry();
    let counted = jobs.clone();
    let file = MetricsFile::new("api", &dir.0, registry).with_refresh(move || {
        counted.get_or_create(&[("kind", "tables")]).inc();
    });
    file.render();
    file.render();
    assert_eq!(jobs.get_or_create(&[("kind", "tables")]).get(), 2);
}

#[test]
fn proc_fields_are_read_after_the_command_name() {
    // A command name with spaces and parentheses, as `comm` may hold.
    let stat = "42 (a (b) c) S 1 42 42 0 -1 4194560 100 0 0 0 250 50 0 0 20 0 3 0 1 2 3";
    assert_eq!(cpu_seconds_of(stat), Some(3.0));
    let status = "Name:\tqd\nVmPeak:\t  2048 kB\nVmRSS:\t  1536 kB\n";
    assert_eq!(resident_bytes_of(status), Some(1536 * 1024));
    assert_eq!(cpu_seconds_of("garbage"), None);
}

#[test]
fn reading_refuses_what_the_text_format_does_not_allow() {
    assert!(samples("qd_x 1\n").is_ok());
    assert!(samples("qd_x{a=\"1\",b=\"q\\\"\"} 2.5\n# EOF\n").is_ok());
    assert!(samples("qd x 1\n").is_err());
    assert!(samples("qd_x{a=1} 1\n").is_err());
    assert!(samples("qd_x{a=\"1\" 1\n").is_err());
    assert!(samples("qd_x one\n").is_err());
    assert!(samples("qd_x 1 1700000000\n").is_err(), "no timestamps");
    assert!(samples("# a free comment\n").is_err());
}
