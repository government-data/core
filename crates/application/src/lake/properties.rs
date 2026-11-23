//! What a lake supports, as its module declares it (decision 0078, point
//! 4): read, never assumed. The core and the workers build no address from
//! another; each is one of these.

use std::time::Duration;

/// Where the catalog's path prefix comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogPrefix {
    /// From the catalog's `GET /v1/config?warehouse=<name>` (RustFS gives
    /// the warehouse's name there).
    FromConfig,
    /// The one the lake declares.
    Fixed(String),
}

/// How objects are addressed on the S3 endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlStyle {
    /// `<endpoint>/<bucket>/<key>`.
    Path,
    /// `<bucket>.<endpoint>/<key>`.
    VirtualHosted,
}

/// The properties a lake's capability declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LakeProperties {
    /// The Iceberg REST catalog's base URI (`https://…/iceberg`).
    pub catalog_uri: String,
    pub catalog_prefix: CatalogPrefix,
    /// The S3 endpoint (`https://host[:port]`).
    pub s3_endpoint: String,
    pub region: String,
    pub url_style: UrlStyle,
    /// The SigV4 service the catalog's requests are signed for (`s3`).
    pub catalog_sigv4_service: String,
    /// The `RoleArn` STS is asked with; `None` when the lake does not read
    /// it (RustFS 1.0.5: any value, or none, is taken alike).
    pub sts_role: Option<String>,
    /// The shortest credentials STS gives (RustFS 1.0.5: 15 minutes; a
    /// shorter one asked is raised to it).
    pub sts_min_duration: Duration,
    /// Whether the catalog vends credentials (RustFS: no).
    pub vends_credentials: bool,
    /// Whether a drop with `purgeRequested` deletes the files (RustFS: no;
    /// it is refused, so the files go first while the catalog still names
    /// the table).
    pub drop_purges_files: bool,
}

impl LakeProperties {
    /// Whether every address is an absolute `http(s)` URL and every name is
    /// given.
    pub fn check(&self) -> Result<(), String> {
        for (what, url) in [
            ("catalog_uri", &self.catalog_uri),
            ("s3_endpoint", &self.s3_endpoint),
        ] {
            let scheme = url.starts_with("https://") || url.starts_with("http://");
            let host = url.split("://").nth(1).is_some_and(|rest| !rest.is_empty());
            if !scheme || !host {
                return Err(format!("{what}: not an http(s) URL"));
            }
        }
        if self.region.trim().is_empty() || self.catalog_sigv4_service.trim().is_empty() {
            return Err("region and catalog_sigv4_service are needed".into());
        }
        if matches!(&self.catalog_prefix, CatalogPrefix::Fixed(p) if p.trim().is_empty()) {
            return Err("catalog_prefix: empty".into());
        }
        if self.sts_min_duration.is_zero() {
            return Err("sts_min_duration: zero".into());
        }
        Ok(())
    }

    /// How long to ask STS for, for credentials wanted for `wanted`: never
    /// below what the lake gives.
    pub fn credentials_duration(&self, wanted: Duration) -> Duration {
        wanted.max(self.sts_min_duration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rustfs() -> LakeProperties {
        LakeProperties {
            catalog_uri: "https://lake.test/iceberg".into(),
            catalog_prefix: CatalogPrefix::FromConfig,
            s3_endpoint: "https://lake.test".into(),
            region: "us-east-1".into(),
            url_style: UrlStyle::Path,
            catalog_sigv4_service: "s3".into(),
            sts_role: None,
            sts_min_duration: Duration::from_secs(900),
            vends_credentials: false,
            drop_purges_files: false,
        }
    }

    #[test]
    fn rustfs_as_verified_is_valid() {
        assert_eq!(rustfs().check(), Ok(()));
    }

    #[test]
    fn an_address_or_a_name_missing_is_refused() {
        let bad = |f: fn(&mut LakeProperties)| {
            let mut p = rustfs();
            f(&mut p);
            p.check().is_err()
        };
        assert!(bad(|p| p.catalog_uri = "lake.test/iceberg".into()));
        assert!(bad(|p| p.s3_endpoint = "https://".into()));
        assert!(bad(|p| p.region = " ".into()));
        assert!(bad(
            |p| p.catalog_prefix = CatalogPrefix::Fixed(String::new())
        ));
        assert!(bad(|p| p.sts_min_duration = Duration::ZERO));
    }

    #[test]
    fn credentials_are_never_asked_shorter_than_the_lake_gives() {
        let p = rustfs();
        let minute = Duration::from_secs(60);
        assert_eq!(p.credentials_duration(minute), Duration::from_secs(900));
        assert_eq!(p.credentials_duration(minute * 60), minute * 60);
    }
}
