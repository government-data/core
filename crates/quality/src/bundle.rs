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
/// fails the job and its data is discarded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Expect {
    /// An `INSERT` or `DELETE`: one `bigint` row, the rows written.
    Write,
    /// One row of `bigint` counts, `columns` of them.
    Counts { columns: u32 },
    /// Rows of these columns, at most `max` (samples and pages, point 7.4;
    /// snapshot ids from `$snapshots`).
    Rows { columns: Vec<String>, max: u32 },
    /// A statement that answers nothing but success (`CREATE`, `ALTER`).
    Done,
}

/// One statement of a bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleStatement {
    /// Its id within the job, as the job runner reports it.
    pub id: String,
    pub sql: String,
    /// How many times the cut is bound to it, in order.
    #[serde(default)]
    pub cut_params: u32,
    pub expects: Expect,
}

/// The job contract (point 2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bundle {
    pub job_id: String,
    pub kind: String,
    pub dataset: String,
    #[serde(default)]
    pub cut: Option<String>,
    pub attempt: u32,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub run_seq: Option<i64>,
    pub catalog_commit: String,
    pub compiler: String,
    pub statements: Vec<BundleStatement>,
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
    if bundle.job_id != job_id || bundle.digest() != digest {
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
            dataset: "loans".into(),
            cut: Some("2026-10-31".into()),
            attempt: 1,
            run_id: Some("r-1".into()),
            run_seq: Some(7),
            catalog_commit: "c0ffee".into(),
            compiler: "quality-rules 0.1.0".into(),
            statements: vec![BundleStatement {
                id: "rules".into(),
                sql: "INSERT INTO t SELECT 1".into(),
                cut_params: 1,
                expects: Expect::Write,
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
        assert!(b.canonical().starts_with("{\"job_id\":"));
        assert!(b.canonical().contains("\"expects\":{\"kind\":\"write\"}"));
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
        let not_a_bundle = key.sign(BUNDLE_TYP, "{\"sub\":\"x\"}");
        assert_eq!(
            verify(&key, &not_a_bundle, &b.job_id, &b.digest()),
            Err(BundleError::Shape)
        );
    }
}
