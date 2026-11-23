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
/// `RUSTFS_STS_ISSUER_SECRET_KEY` (or its `_FILE`), trusting `LAKE_CA_FILE`
/// and `LAKE_CLIENT_CERT_FILE` too if set. `None` without an endpoint or
/// keys.
pub fn issuer_from_env() -> Result<Option<StsIssuer>, String> {
    let var = |n: &str| std::env::var(n).ok().filter(|v| !v.trim().is_empty());
    let (Some(endpoint), Some(access)) =
        (var("LAKE_ENDPOINT"), var("RUSTFS_STS_ISSUER_ACCESS_KEY"))
    else {
        return Ok(None);
    };
    let secret = key_from_env("RUSTFS_STS_ISSUER_SECRET_KEY")?
        .ok_or("RUSTFS_STS_ISSUER_ACCESS_KEY needs RUSTFS_STS_ISSUER_SECRET_KEY")?;
    let http = LakeClient::from_env(Duration::from_secs(10))?;
    Ok(Some(StsIssuer::new(
        http,
        properties(&endpoint),
        &access,
        &secret,
    )))
}

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
}
