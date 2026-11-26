//! What RustFS declares as a lake (decision 0078, point 4), until its
//! module's manifest declares it (step 17), and the platform's issuer as
//! the installation configures it today: `LAKE_ENDPOINT` and
//! `RUSTFS_STS_ISSUER_*`. Verified with RustFS 1.0.5
//! (`tools/verify/lake_contract_rustfs.sh`): the catalog under
//! `/iceberg`, its prefix from `/v1/config`, SigV4 service `s3`, no role
//! read by STS, 15 minutes at least, no vended credentials, no purge.

use std::time::Duration;

use application::lake::{CatalogPrefix, LakeProperties, UrlStyle};

use crate::lake_http::LakeClient;
use crate::lake_sts::StsIssuer;
use crate::oidc_es256::key_from_env;

/// RustFS's properties, for a lake whose S3 endpoint is `endpoint`.
pub fn properties(endpoint: &str) -> LakeProperties {
    let endpoint = endpoint.trim_end_matches('/');
    LakeProperties {
        catalog_uri: format!("{endpoint}/iceberg"),
        catalog_prefix: CatalogPrefix::FromConfig,
        s3_endpoint: endpoint.to_string(),
        region: sigv4::REGION.into(),
        url_style: UrlStyle::Path,
        catalog_sigv4_service: "s3".into(),
        sts_role: None,
        sts_min_duration: Duration::from_secs(900),
        vends_credentials: false,
        drop_purges_files: false,
    }
}

/// The issuer from `LAKE_ENDPOINT`, `RUSTFS_STS_ISSUER_ACCESS_KEY` and
/// `RUSTFS_STS_ISSUER_SECRET_KEY` (each or its `_FILE`, as the package
/// passes them: decision 0091, point 4), trusting `LAKE_CA_FILE` and
/// `LAKE_CLIENT_CERT_FILE` too if set. `None` without an endpoint or keys.
pub fn issuer_from_env() -> Result<Option<StsIssuer>, String> {
    let endpoint = std::env::var("LAKE_ENDPOINT")
        .ok()
        .filter(|v| !v.trim().is_empty());
    let Some(endpoint) = endpoint else {
        return Ok(None);
    };
    let Some((access, secret)) = issuer_keys_from_env()? else {
        return Ok(None);
    };
    let http = LakeClient::from_env(Duration::from_secs(10))?;
    Ok(Some(StsIssuer::new(
        http,
        properties(&endpoint),
        &access,
        &secret,
    )))
}

/// The issuer's access and secret keys, from the variables or their
/// files; `None` without an access key, an error with one but no secret.
/// The core makes the lake's user of these keys at start (ruling 51).
pub fn issuer_keys_from_env() -> Result<Option<(String, String)>, String> {
    let Some(access) = key_from_env(ACCESS_KEY)? else {
        return Ok(None);
    };
    let secret = key_from_env(SECRET_KEY)?
        .ok_or("RUSTFS_STS_ISSUER_ACCESS_KEY needs RUSTFS_STS_ISSUER_SECRET_KEY")?;
    Ok(Some((access, secret)))
}

const ACCESS_KEY: &str = "RUSTFS_STS_ISSUER_ACCESS_KEY";
const SECRET_KEY: &str = "RUSTFS_STS_ISSUER_SECRET_KEY";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rustfs_as_verified() {
        let p = properties("https://lake.test/");
        assert_eq!(p.check(), Ok(()));
        assert_eq!(p.catalog_uri, "https://lake.test/iceberg");
        assert_eq!(p.sts_role, None);
        assert!(!p.vends_credentials && !p.drop_purges_files);
    }

    /// The only test that sets the issuer's variables, so no other test
    /// races it.
    #[test]
    fn keys_from_the_variables_or_their_files() {
        let dir = std::env::temp_dir().join(format!("qd-sts-keys-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (access_file, secret_file) = (dir.join("access"), dir.join("secret"));
        std::fs::write(&access_file, "qd-access-file\n").unwrap();
        std::fs::write(&secret_file, "secret-from-file").unwrap();
        let clear = || {
            for n in [ACCESS_KEY, SECRET_KEY] {
                std::env::remove_var(n);
                std::env::remove_var(format!("{n}_FILE"));
            }
        };
        clear();
        assert_eq!(issuer_keys_from_env(), Ok(None));

        std::env::set_var(format!("{ACCESS_KEY}_FILE"), &access_file);
        assert!(issuer_keys_from_env().unwrap_err().contains("needs"));
        std::env::set_var(format!("{SECRET_KEY}_FILE"), &secret_file);
        let keys = issuer_keys_from_env().unwrap().unwrap();
        assert_eq!(keys, ("qd-access-file".into(), "secret-from-file".into()));

        // A variable wins over its file, as for every key.
        std::env::set_var(ACCESS_KEY, "qd-access-var");
        assert_eq!(issuer_keys_from_env().unwrap().unwrap().0, "qd-access-var");

        std::env::set_var(format!("{ACCESS_KEY}_FILE"), dir.join("missing"));
        std::env::remove_var(ACCESS_KEY);
        assert!(issuer_keys_from_env().unwrap_err().contains("_FILE"));
        clear();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
