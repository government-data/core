use sha2::{Digest, Sha256};

use super::*;

const IMAGE: &str =
    "123456789012.dkr.ecr.us-east-1.amazonaws.com/quality-data/lake-rustfs/rustfs@sha256:\
0000000000000000000000000000000000000000000000000000000000000000";

pub(crate) fn role(name: &str) -> ModuleRole {
    ModuleRole {
        name: name.into(),
        strategy: RoleStrategy::All,
        services: vec!["rustfs".into(), "gate".into()],
        images: vec![IMAGE.into()],
        files: Vec::new(),
    }
}

pub(crate) fn file(path: &str, content: &[u8]) -> RoleFile {
    let sha256 = Sha256::digest(content)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    RoleFile {
        path: path.into(),
        sha256,
        content_base64: base64::engine::general_purpose::STANDARD.encode(content),
    }
}

#[test]
fn a_role_with_its_images_and_files_passes() {
    let mut lake = role("lake");
    lake.files = vec![file("compose.yml", b"services: {}\n")];
    let mut replica = role("lake-replica");
    replica.strategy = RoleStrategy::One;
    replica.files = lake.files.clone();
    assert_eq!(check(&[lake, replica]), Ok(()));
    assert_eq!(check(&[]), Ok(()), "no roles");
}

#[test]
fn the_wire_reads_one_and_all_and_files_are_optional() {
    let wire = serde_json::json!({
        "name": "lake", "strategy": "one", "services": ["rustfs"], "images": [IMAGE],
    });
    let read: ModuleRole = serde_json::from_value(wire).unwrap();
    assert_eq!(read.strategy, RoleStrategy::One);
    assert!(read.files.is_empty());
    let rolling = serde_json::json!({
        "name": "lake", "strategy": "rolling", "services": ["rustfs"], "images": [IMAGE],
    });
    assert!(serde_json::from_value::<ModuleRole>(rolling).is_err());
}

#[test]
fn bad_names_and_lists_are_refused() {
    let named = |n: &str| check(&[role(n)]);
    assert_eq!(named("Lake"), Err(Refusal::BadName("Lake".into())));
    assert_eq!(named("-lake"), Err(Refusal::BadName("-lake".into())));
    assert!(matches!(named(&"a".repeat(64)), Err(Refusal::BadName(_))));
    assert_eq!(named("app"), Err(Refusal::Reserved("app".into())));
    assert_eq!(named("updater"), Err(Refusal::Reserved("updater".into())));
    assert_eq!(
        check(&[role("lake"), role("lake")]),
        Err(Refusal::Repeated("lake".into()))
    );
    let many: Vec<_> = (0..17).map(|i| role(&format!("r{i}"))).collect();
    assert_eq!(check(&many), Err(Refusal::TooMany));
    for services in [
        vec![],
        vec!["gate".into(), "gate".into()],
        vec!["a b".into()],
    ] {
        let r = ModuleRole {
            services,
            ..role("lake")
        };
        assert_eq!(check(&[r]), Err(Refusal::BadServices("lake".into())));
    }
}

#[test]
fn an_image_not_pinned_by_digest_is_refused() {
    for image in [
        "rustfs:latest",
        "host/quality-data/rustfs:1.0",
        "host/quality-data/rustfs@sha256:abc",
        "https://host/rustfs@sha256:0000000000000000000000000000000000000000000000000000000000000000",
        "/rustfs@sha256:0000000000000000000000000000000000000000000000000000000000000000",
    ] {
        let r = ModuleRole {
            images: vec![image.into()],
            ..role("lake")
        };
        assert_eq!(check(&[r]), Err(Refusal::BadImages("lake".into())), "{image}");
    }
    let none = ModuleRole {
        images: vec![],
        ..role("lake")
    };
    assert_eq!(check(&[none]), Err(Refusal::BadImages("lake".into())));
}

#[test]
fn a_file_outside_the_module_or_not_matching_its_digest_is_refused() {
    let mut wrong = file("compose.yml", b"a");
    wrong.content_base64 = base64::engine::general_purpose::STANDARD.encode(b"b");
    let mut not_base64 = file("compose.yml", b"a");
    not_base64.content_base64 = "***".into();
    for bad in [
        file("../etc/passwd", b"a"),
        file("/etc/passwd", b"a"),
        file("config//gate.toml", b"a"),
        wrong,
        not_base64,
    ] {
        let mut r = role("lake");
        r.files = vec![bad];
        assert_eq!(check(&[r]), Err(Refusal::BadFile("lake".into())));
    }
}

#[test]
fn a_modules_files_weigh_at_most_256_kib_each_counted_once() {
    let half = vec![b'x'; MAX_MODULE_FILES_BYTES / 2];
    let mut lake = role("lake");
    lake.files = vec![file("a.bin", &half), file("b.bin", &half)];
    let mut replica = role("lake-replica");
    replica.files = lake.files.clone();
    assert_eq!(
        check(&[lake.clone(), replica]),
        Ok(()),
        "the same files twice"
    );
    lake.files.push(file("c.bin", b"one byte too many"));
    assert_eq!(check(&[lake]), Err(Refusal::FilesTooLarge));
}

#[test]
fn a_files_debug_does_not_show_its_content() {
    let shown = format!("{:?}", file("compose.yml", b"password: hunter2"));
    assert!(shown.contains("compose.yml"));
    assert!(
        !shown.contains(&base64::engine::general_purpose::STANDARD.encode(b"password: hunter2"))
    );
}
