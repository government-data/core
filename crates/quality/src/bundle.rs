//! A job's signed statement bundle (decision 0090, point 2): every
//! statement a job runner may submit for one job, compiled by the core and
//! signed with the OIDC provider's ES256 key, which job runners already
//! verify (ruled by the user 2026-11-25). The module reaches the key
//! through [`BundleSigner`], a port the server wires to the provider; it
//! never depends on `auth-gateway`. A statement not in a verified bundle
//! is never run.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The JWS `typ` of a bundle, so no other token passes for one.
pub const BUNDLE_TYP: &str = "qd-bundle+jwt";

/// What a statement's answer must look like (point 7.3); anything else
/// fails the job and its data is discarded. The wire shape is the one
/// `engine-trino`'s job runner proposed and the user accepted (2026-11-25).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Expect {
    /// A write: one of these update types (`INSERT`, `DELETE`) with one
    /// `bigint` row, the rows written.
    Count { update_types: Vec<String> },
    /// A read: exactly these columns, as `(name, Trino type)`, at most
    /// `max_rows` rows (counts read back, samples and pages, point 7.4;
    /// snapshot ids from `$snapshots`).
    Rows {
        columns: Vec<(String, String)>,
        max_rows: u64,
    },
    /// A statement that answers nothing but success (`CREATE`, `ALTER`).
    Done,
}

impl Expect {
    /// An `INSERT`'s count.
    pub fn insert() -> Self {
        Self::Count {
            update_types: vec!["INSERT".into()],
        }
    }

    /// A `DELETE`'s count.
    pub fn delete() -> Self {
        Self::Count {
            update_types: vec!["DELETE".into()],
        }
    }
}

/// Who a statement runs as: the job's own user, or its dataset's report
/// user, which reads gold only (point 7.3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunAs {
    #[default]
    Job,
    Report,
}

/// One statement of a bundle. Its SQL is complete: nothing is bound to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleStatement {
    /// Its id within the job (`[a-z0-9_]`, at most 64), as the job runner
    /// reports it.
    pub id: String,
    pub sql: String,
    pub expects: Expect,
    #[serde(default)]
    pub run_as: RunAs,
    /// Whether it reads the source, and so takes the source's secrets as
    /// extra credentials (point 7.5).
    #[serde(default)]
    pub reads_source: bool,
}

/// The job contract (point 2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bundle {
    pub job_id: String,
    pub kind: String,
    pub dataset: Option<String>,
    pub cut: Option<String>,
    pub attempt: u32,
    pub run_id: Option<String>,
    pub run_seq: Option<u64>,
    pub catalog_commit: Option<String>,
    pub compiler_version: String,
    pub statements: Vec<BundleStatement>,
}

/// Whether `id` is a statement id the job runner accepts.
pub fn is_statement_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

impl Bundle {
    /// Its canonical JSON: the fields in the order declared, no spaces.
    pub fn canonical(&self) -> String {
        serde_json::to_string(self).expect("a bundle serializes")
    }

    /// The digest the operator carries with the job: the lower-case hex
    /// SHA-256 of the canonical JSON.
    pub fn digest(&self) -> String {
        format!("{:x}", Sha256::digest(self.canonical().as_bytes()))
    }

    /// Whether the job runner will take it: statements, each with a well
    /// formed id used once.
    pub fn well_formed(&self) -> bool {
        let mut ids = std::collections::BTreeSet::new();
        !self.statements.is_empty()
            && self
                .statements
                .iter()
                .all(|s| is_statement_id(&s.id) && ids.insert(s.id.as_str()))
    }
}

/// The port to the provider's signing key.
pub trait BundleSigner: Send + Sync {
    /// A compact JWS of `claims` (a JSON object) with `typ` in its header.
    fn sign(&self, typ: &str, claims: &str) -> String;
    /// The claims of a JWS this key signed with `typ`, if it verifies.
    fn verify(&self, typ: &str, token: &str) -> Option<String>;
}

/// Why a bundle was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleError {
    /// The signature, the key or the `typ` does not check out.
    Signature,
    /// It verified but is not a bundle.
    Shape,
    /// Not the job's, or not the digest the operator carries.
    NotThisJob,
}

/// The signed bundle.
pub fn sign(signer: &dyn BundleSigner, bundle: &Bundle) -> String {
    signer.sign(BUNDLE_TYP, &bundle.canonical())
}

/// A signed bundle checked as a job runner checks it: the signature, then
/// that it is the job's and has the digest the operator handed out.
pub fn verify(
    signer: &dyn BundleSigner,
    token: &str,
    job_id: &str,
    digest: &str,
) -> Result<Bundle, BundleError> {
    let claims = signer
        .verify(BUNDLE_TYP, token)
        .ok_or(BundleError::Signature)?;
    let bundle: Bundle = serde_json::from_str(&claims).map_err(|_| BundleError::Shape)?;
    if bundle.job_id != job_id || bundle.digest() != digest || !bundle.well_formed() {
        return Err(BundleError::NotThisJob);
    }
    Ok(bundle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;

    /// A stand-in key: `typ.payload.mac`, the MAC a keyed SHA-256. The real
    /// one is the provider's ES256 key, tested where the server wires it.
    struct Fake(&'static str);

    impl Fake {
        fn mac(&self, typ: &str, payload: &str) -> String {
            format!(
                "{:x}",
                Sha256::digest(format!("{}|{typ}|{payload}", self.0).as_bytes())
            )
        }
    }

    impl BundleSigner for Fake {
        fn sign(&self, typ: &str, claims: &str) -> String {
            let payload = URL_SAFE_NO_PAD.encode(claims);
            format!("{typ}.{payload}.{}", self.mac(typ, &payload))
        }
        fn verify(&self, typ: &str, token: &str) -> Option<String> {
            let mut parts = token.split('.');
            let (t, payload, mac) = (parts.next()?, parts.next()?, parts.next()?);
            (t == typ && mac == self.mac(typ, payload))
                .then(|| String::from_utf8(URL_SAFE_NO_PAD.decode(payload).ok()?).ok())?
        }
    }

    fn bundle() -> Bundle {
        Bundle {
            job_id: "0b9c6a1e-0000-4000-8000-000000000001".into(),
            kind: "evaluate".into(),
            dataset: Some("loans".into()),
            cut: Some("2026-10-31".into()),
            attempt: 1,
            run_id: Some("r-1".into()),
            run_seq: Some(7),
            catalog_commit: Some("c0ffee".into()),
            compiler_version: "quality-rules 0.1.0".into(),
            statements: vec![BundleStatement {
                id: "rules".into(),
                sql: "INSERT INTO t SELECT 1".into(),
                expects: Expect::insert(),
                run_as: RunAs::Job,
                reads_source: false,
            }],
        }
    }

    #[test]
    fn a_signed_bundle_round_trips_for_its_job_and_digest() {
        let key = Fake("k1");
        let b = bundle();
        let token = sign(&key, &b);
        assert_eq!(verify(&key, &token, &b.job_id, &b.digest()), Ok(b.clone()));
        assert_eq!(b.digest().len(), 64);
        let rows = Expect::Rows {
            columns: vec![("n".into(), "bigint".into())],
            max_rows: 1,
        };
        assert_eq!(
            serde_json::to_string(&rows).unwrap(),
            "{\"kind\":\"rows\",\"columns\":[[\"n\",\"bigint\"]],\"max_rows\":1}"
        );
        assert!(b.canonical().starts_with("{\"job_id\":"));
        assert!(b.canonical().contains(
            "\"expects\":{\"kind\":\"count\",\"update_types\":[\"INSERT\"]},\"run_as\":\"job\""
        ));
    }

    #[test]
    fn a_tampered_bundle_another_key_or_another_job_is_refused() {
        let key = Fake("k1");
        let b = bundle();
        let token = sign(&key, &b);
        let mut evil = b.clone();
        evil.statements[0].sql = "SELECT * FROM lake.loans_silver.loans".into();
        let payload = URL_SAFE_NO_PAD.encode(evil.canonical());
        let parts: Vec<&str> = token.split('.').collect();
        let tampered = format!("{}.{payload}.{}", parts[0], parts[2]);
        assert_eq!(
            verify(&key, &tampered, &b.job_id, &b.digest()),
            Err(BundleError::Signature)
        );
        assert_eq!(
            verify(&Fake("k2"), &token, &b.job_id, &b.digest()),
            Err(BundleError::Signature)
        );
        assert_eq!(
            verify(&key, &token, "another-job", &b.digest()),
            Err(BundleError::NotThisJob)
        );
        assert_eq!(
            verify(&key, &token, &b.job_id, &evil.digest()),
            Err(BundleError::NotThisJob)
        );
        let other = key.sign("JWT", &b.canonical());
        assert_eq!(
            verify(&key, &other, &b.job_id, &b.digest()),
            Err(BundleError::Signature)
        );
        let mut repeated = b.clone();
        repeated.statements.push(b.statements[0].clone());
        let token = sign(&key, &repeated);
        assert_eq!(
            verify(&key, &token, &b.job_id, &repeated.digest()),
            Err(BundleError::NotThisJob),
            "a repeated statement id"
        );
        let not_a_bundle = key.sign(BUNDLE_TYP, "{\"sub\":\"x\"}");
        assert_eq!(
            verify(&key, &not_a_bundle, &b.job_id, &b.digest()),
            Err(BundleError::Shape)
        );
    }
}
