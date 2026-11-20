//! What the tests of a machine's certificate share (0066): a key and a
//! signing request made in the test, a machine that joins, and the header
//! as Traefik writes it (URL-encoded, comma-separated base64 DER, the
//! verified one first).
#![allow(dead_code)]

use application::{MachinesService, Origin};
use auth_gateway::MachineRole;
use domain::AdminId;
use rcgen::{CertificateParams, DnType, KeyPair, PKCS_ECDSA_P256_SHA256};

/// A machine: its key and its certificate (PEM).
pub(crate) struct Held {
    pub(crate) key: KeyPair,
    pub(crate) cert: String,
}

pub(crate) fn key() -> KeyPair {
    KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap()
}

/// A request of `key`, asking for a subject the CA must not give.
pub(crate) fn csr(key: &KeyPair) -> String {
    let mut params = CertificateParams::new(vec!["evil.example".to_string()]).unwrap();
    params.distinguished_name.push(DnType::CommonName, "root");
    params
        .distinguished_name
        .push(DnType::OrganizationalUnitName, "admin");
    params.serialize_request(key).unwrap().pem().unwrap()
}

pub(crate) fn origin() -> Origin {
    Origin {
        ip: "192.0.2.1".into(),
        user_agent: None,
    }
}

pub(crate) fn admin() -> AdminId {
    AdminId::new("00000000-0000-0000-0000-0000000000ad")
}

/// A machine joins, as `/install/join` does: pending, with its certificate.
pub(crate) async fn joined(machines: &MachinesService, role: MachineRole, name: &str) -> Held {
    let (by, origin) = (admin(), origin());
    let issued = machines.issue_join_token(role, None, &by, &origin);
    let token = issued.await.unwrap().token;
    let key = key();
    let request = csr(&key);
    let joined = machines.join(&token, name, &request, &origin);
    let cert = joined.await.unwrap().certificate;
    Held { key, cert }
}

/// A lake node of the pool `main` joins under its name, as a pool's node
/// does since 0066 step 11: the pool defined first (with `name` among its
/// four nodes), a token naming the node, then the join. Needs the service
/// built with a pool store.
pub(crate) async fn joined_node(machines: &MachinesService, name: &str) -> Held {
    let (by, origin) = (admin(), origin());
    let nodes: Vec<String> = [name, "lake-node-b", "lake-node-c", "lake-node-d"]
        .map(String::from)
        .to_vec();
    let defined = machines.define_pool("main", &nodes, 1, &by, &origin);
    defined.await.unwrap();
    let issued = machines.issue_node_token("main", name, 0, &by, &origin);
    let token = issued.await.unwrap().token;
    let key = key();
    let request = csr(&key);
    let joined = machines.join(&token, name, &request, &origin);
    let cert = joined.await.unwrap().certificate;
    Held { key, cert }
}

/// The header Traefik writes for the certificates a client presented.
pub(crate) fn traefik_header(pems: &[&str]) -> String {
    let b64 = |pem: &&str| -> String { pem.lines().filter(|l| !l.starts_with("-----")).collect() };
    let joined = pems.iter().map(b64).collect::<Vec<_>>().join(",");
    joined
        .replace('+', "%2B")
        .replace('/', "%2F")
        .replace('=', "%3D")
        .replace(',', "%2C")
}
