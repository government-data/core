//! The contract against the in-memory fake (the user's ruling of
//! 2026-11-23): the cases over the two ports, and every `lake-admin/1`
//! case at the wire with the fake served in process behind mTLS.

use std::sync::Arc;

use lake_contract::admin_wire::{self, AdminClient};
use lake_contract::ports;
use testkit::lake::InMemoryLake;
use testkit::lake_admin::{serve, CALLER_ROLE};
use testkit::tls::TestCa;

const WAREHOUSE: &str = "qd-tenant-00000000000000000000000000000001";

#[tokio::test]
async fn the_fake_passes_the_ports_cases() {
    let lake = InMemoryLake::default();
    ports::lake_admin(&lake, WAREHOUSE).await;
    ports::lake(&lake, WAREHOUSE).await;
}

#[tokio::test]
async fn the_fake_passes_lake_admin_1_at_the_wire() {
    let ca = TestCa::new();
    let server = serve(Arc::new(InMemoryLake::default()), &ca).await;
    let core = AdminClient::new(
        &server.url,
        &ca.pem,
        &ca.machine(CALLER_ROLE, "app-1").bundle(),
    );
    admin_wire::operations_twice(&core, WAREHOUSE).await;
    admin_wire::errors(&core, "qd-tenant-unknown", &[]).await;

    let before = server.lake.snapshot();
    let worker = AdminClient::new(&server.url, &ca.pem, &ca.machine("worker", "w-1").bundle());
    admin_wire::refused_by_its_certificate(&worker, "qd-tenant-other").await;
    assert_eq!(
        server.lake.snapshot(),
        before,
        "a refused call changes nothing"
    );
}
