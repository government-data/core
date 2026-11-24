//! The replication of the main lake into the replica (0028's `LakeAdmin`,
//! its replication; 0066 step 11, the user's rulings of 2026-11-13) over
//! RustFS's S3 and admin APIs, each call signed with the lake's platform
//! admin keys, which [`RustfsLakeAdmin`] alone opens. Verified with RustFS
//! 1.0.5 (`tools/verify/rustfs_replication_gate.sh`,
//! `rustfs_replication_lifecycle.sh`): every step answers 200 again on a
//! second run and changes nothing, a bucket made again answers 200, the
//! target set again answers the same ARN, and no `Content-MD5` is needed
//! with the payload's hash signed. No key or signature is logged; the
//! replica's keys travel only in the remote target's body, to the source.

use auth_gateway::{
    Lake, LakeAdminKeys, ReplicationOutcome, ReplicationStep, REPLICATION_ALIAS,
    REPLICATION_PROXY_PORT, REPLICA_RETENTION_DAYS, SOURCE_NONCURRENT_DAYS,
};
use chrono::Utc;
use domain::RepositoryError;
use ReplicationStep::*;

use crate::rustfs_admin::{chain, code, RustfsLakeAdmin};

const S3_XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";
/// The rule's id, so a person reading the bucket knows whose it is.
pub const RULE_ID: &str = "qd-replica";
/// The lifecycle rule's id.
pub const LIFECYCLE_ID: &str = "qd-noncurrent";

/// What one warehouse's replication is configured with. Holds no key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WarehouseReplication {
    /// The bucket, the same on both lakes.
    pub warehouse: String,
    /// Where the source answers: its base URL (the main lake's,
    /// `LAKE_ENDPOINT`).
    pub source: String,
    /// The replica's node, by its name (its gate's port).
    pub replica: String,
    /// The remote target as the source reaches it: [`proxy_of`].
    pub proxy: String,
}

/// Where the source reaches its replication proxy: a pool's nodes each
/// their own, by the alias they all map to their loopback (0066 step 11,
/// ruling 5); a single node's, its own name; on
/// [`REPLICATION_PROXY_PORT`]. `None` if `source` names no host.
pub fn proxy_of(source: &str, pool: bool) -> Option<String> {
    let rest = source.split_once("://").map_or(source, |(_, r)| r);
    let authority = rest.split('/').next()?;
    let host = authority.rsplit_once(':').map_or(authority, |(h, _)| h);
    let host = if pool { REPLICATION_ALIAS } else { host };
    (!host.is_empty()).then(|| format!("{host}:{REPLICATION_PROXY_PORT}"))
}

/// One signed call: where, what, and the body.
pub(crate) struct Call<'a> {
    base: &'a str,
    method: &'a str,
    path: String,
    query: Vec<(&'a str, String)>,
    headers: Vec<(&'a str, &'a str)>,
    body: Vec<u8>,
}

/// The replica's default retention, `COMPLIANCE`.
pub fn retention_xml() -> String {
    format!(
        "<ObjectLockConfiguration xmlns=\"{S3_XMLNS}\"><ObjectLockEnabled>Enabled</ObjectLockEnabled>\
         <Rule><DefaultRetention><Mode>COMPLIANCE</Mode><Days>{REPLICA_RETENTION_DAYS}</Days>\
         </DefaultRetention></Rule></ObjectLockConfiguration>"
    )
}

/// Versioning on the source.
pub fn versioning_xml() -> String {
    format!("<VersioningConfiguration xmlns=\"{S3_XMLNS}\"><Status>Enabled</Status></VersioningConfiguration>")
}

/// The source's noncurrent versions expire.
pub fn lifecycle_xml() -> String {
    format!(
        "<LifecycleConfiguration xmlns=\"{S3_XMLNS}\"><Rule><ID>{LIFECYCLE_ID}</ID>\
         <Filter><Prefix></Prefix></Filter><Status>Enabled</Status><NoncurrentVersionExpiration>\
         <NoncurrentDays>{SOURCE_NONCURRENT_DAYS}</NoncurrentDays></NoncurrentVersionExpiration>\
         </Rule></LifecycleConfiguration>"
    )
}

/// The rule: everything, to the target, without delete markers.
pub fn rule_xml(arn: &str) -> String {
    let arn = xml_escape(arn);
    format!(
        "<ReplicationConfiguration xmlns=\"{S3_XMLNS}\"><Role>{arn}</Role><Rule><ID>{RULE_ID}</ID>\
         <Status>Enabled</Status><Priority>1</Priority><Filter><Prefix></Prefix></Filter>\
         <DeleteMarkerReplication><Status>Disabled</Status></DeleteMarkerReplication>\
         <Destination><Bucket>{arn}</Bucket></Destination></Rule></ReplicationConfiguration>"
    )
}

/// The remote target's body: the replica's keys, given to the source. A
/// pool's proxy, reached by [`REPLICATION_ALIAS`] on each node's own
/// loopback, listens without TLS (0066 step 11, ruling 5); a single node's
/// serves TLS with its certificate.
pub fn target_json(w: &WarehouseReplication, replica: &LakeAdminKeys) -> String {
    let pooled = w.proxy.split(':').next() == Some(REPLICATION_ALIAS);
    serde_json::json!({
        "endpoint": w.proxy,
        "secure": !pooled,
        "targetbucket": w.warehouse,
        "type": "replication",
        "region": sigv4::REGION,
        "credentials": { "accessKey": replica.access_key, "secretKey": replica.secret_key },
    })
    .to_string()
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// What a call answered, or why it was not answered.
pub(crate) type Answer = Result<(u16, String), String>;

impl RustfsLakeAdmin {
    pub(crate) async fn send(&self, keys: &LakeAdminKeys, call: Call<'_>) -> Answer {
        let url = format!("{}{}", call.base, call.path);
        let host = sigv4::host(&url).ok_or_else(|| format!("not a lake's address: {url}"))?;
        let hash = sigv4::payload_hash(&call.body);
        let length = call.body.len().to_string();
        let mut headers = vec![
            ("content-length", length.as_str()),
            ("host", host.as_str()),
            ("x-amz-content-sha256", hash.as_str()),
        ];
        headers.extend(call.headers.iter().copied());
        let query: Vec<(&str, &str)> = call.query.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let request = sigv4::Request {
            method: call.method,
            path: &call.path,
            query: &query,
            headers: &headers,
        };
        let signed = sigv4::sign_request(
            "s3",
            &keys.access_key,
            &keys.secret_key,
            &request,
            Utc::now(),
        );
        let qs = sigv4::query_string(&query);
        let full = if qs.is_empty() {
            url
        } else {
            format!("{url}?{qs}")
        };
        let method =
            reqwest::Method::from_bytes(call.method.as_bytes()).map_err(|e| e.to_string())?;
        let mut req = self.http.get().request(method, &full);
        for (name, value) in headers.iter().filter(|(n, _)| *n != "host") {
            req = req.header(*name, *value);
        }
        let sent = req
            .header("x-amz-date", &signed.date)
            .header("authorization", &signed.authorization)
            .body(call.body)
            .send()
            .await
            .map_err(|e| chain(&e))?;
        let status = sent.status().as_u16();
        let text = sent.text().await.map_err(|e| chain(&e))?;
        Ok((status, text))
    }
}

/// A step's answer: 200 passes; anything else fails it, in a few words.
fn passed(step: ReplicationStep, answer: Answer) -> Result<String, ReplicationOutcome> {
    let fail = |detail: String| ReplicationOutcome::Failed { step, detail };
    match answer {
        Ok((200, text)) => Ok(text),
        Ok((status, text)) => {
            let code = code(&text);
            Err(fail(format!("HTTP {status} {code}").trim_end().to_string()))
        }
        Err(detail) => Err(fail(detail)),
    }
}

pub(crate) fn call<'a>(
    base: &'a str,
    method: &'a str,
    path: String,
    query: &[(&'a str, &str)],
) -> Call<'a> {
    Call {
        base,
        method,
        path,
        query: query.iter().map(|(k, v)| (*k, v.to_string())).collect(),
        headers: Vec::new(),
        body: Vec::new(),
    }
}

/// A `PUT` of a bucket's configuration `sub` (`?versioning`, …) with `body`.
fn put_xml<'a>(base: &'a str, bucket: &str, sub: &'a str, body: String) -> Call<'a> {
    let mut c = call(base, "PUT", bucket.to_string(), &[(sub, "")]);
    c.body = body.into_bytes();
    c
}

impl RustfsLakeAdmin {
    async fn steps(
        &self,
        w: &WarehouseReplication,
        source: &LakeAdminKeys,
        replica: &LakeAdminKeys,
    ) -> Result<ReplicationOutcome, ReplicationOutcome> {
        let (src, dst) = (self.base(&w.source), self.base(&w.replica));
        let bucket = format!("/{}", w.warehouse);
        match self
            .send(source, call(&src, "HEAD", bucket.clone(), &[]))
            .await
        {
            Ok((404, _)) => return Ok(ReplicationOutcome::NoWarehouse),
            other => passed(SourceBucket, other)?,
        };
        let mut make = call(&dst, "PUT", bucket.clone(), &[]);
        make.headers
            .push(("x-amz-bucket-object-lock-enabled", "true"));
        passed(ReplicaBucket, self.send(replica, make).await)?;
        let lock = put_xml(&dst, &bucket, "object-lock", retention_xml());
        passed(ReplicaRetention, self.send(replica, lock).await)?;
        let versioning = put_xml(&src, &bucket, "versioning", versioning_xml());
        passed(SourceVersioning, self.send(source, versioning).await)?;
        let lifecycle = put_xml(&src, &bucket, "lifecycle", lifecycle_xml());
        passed(SourceLifecycle, self.send(source, lifecycle).await)?;
        let path = "/rustfs/admin/v3/set-remote-target".to_string();
        let mut target = call(&src, "PUT", path, &[("bucket", &w.warehouse)]);
        target.body = target_json(w, replica).into_bytes();
        let arn = passed(RemoteTarget, self.send(source, target).await)?;
        let arn = arn.trim().trim_matches('"').to_string();
        if !arn.starts_with("arn:") {
            let detail = "no ARN in the answer".to_string();
            return Err(ReplicationOutcome::Failed {
                step: RemoteTarget,
                detail,
            });
        }
        let rule = put_xml(&src, &bucket, "replication", rule_xml(&arn));
        passed(Rule, self.send(source, rule).await)?;
        Ok(ReplicationOutcome::Configured)
    }
}

impl RustfsLakeAdmin {
    /// Configures a warehouse's replication, signed with each lake's kept
    /// admin keys.
    pub async fn configure(
        &self,
        w: &WarehouseReplication,
    ) -> Result<ReplicationOutcome, RepositoryError> {
        let Some(source) = self.keys.open(&Lake::main()).await? else {
            return Ok(ReplicationOutcome::NoKeys { lake: Lake::main() });
        };
        let Some(replica) = self.keys.open(&Lake::Replica).await? else {
            return Ok(ReplicationOutcome::NoKeys {
                lake: Lake::Replica,
            });
        };
        Ok(self.steps(w, &source, &replica).await.unwrap_or_else(|o| o))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bodies_say_what_was_ruled() {
        assert!(retention_xml().contains("<Mode>COMPLIANCE</Mode><Days>30</Days>"));
        assert!(lifecycle_xml().contains("<NoncurrentDays>7</NoncurrentDays>"));
        assert!(versioning_xml().contains("<Status>Enabled</Status>"));
        let rule = rule_xml("arn:minio:replication:us-east-1:x:qd-tenant-a");
        assert!(rule.contains("<DeleteMarkerReplication><Status>Disabled</Status>"));
        assert!(rule.contains("<Role>arn:minio:replication:us-east-1:x:qd-tenant-a</Role>"));
        assert!(rule.contains(
            "<Destination><Bucket>arn:minio:replication:us-east-1:x:qd-tenant-a</Bucket>"
        ));
        assert!(rule_xml("a<b&c").contains("<Role>a&lt;b&amp;c</Role>"));
    }

    #[test]
    fn the_proxy_is_the_source_nodes_name_or_a_pools_alias() {
        assert_eq!(
            proxy_of("https://lake-1:9000", false).as_deref(),
            Some("lake-1:9100")
        );
        assert_eq!(
            proxy_of("https://lake-1.internal/", false).as_deref(),
            Some("lake-1.internal:9100")
        );
        assert_eq!(
            proxy_of("lake-1:9000", false).as_deref(),
            Some("lake-1:9100")
        );
        assert_eq!(
            proxy_of("https://lake-1:9000", true).as_deref(),
            Some("qd-lake-replication:9100")
        );
        assert_eq!(proxy_of("https://", false), None);
    }

    #[test]
    fn a_pools_target_is_its_alias_without_tls_and_a_single_nodes_its_name_with() {
        let keys = LakeAdminKeys {
            access_key: "qd-platform-admin".into(),
            secret_key: "0".repeat(64),
        };
        let target = |proxy: &str| {
            let w = WarehouseReplication {
                warehouse: "qd-tenant-a".into(),
                source: "https://lake-1:9000".into(),
                replica: "replica-1".into(),
                proxy: proxy.into(),
            };
            let body: serde_json::Value = serde_json::from_str(&target_json(&w, &keys)).unwrap();
            (body["endpoint"].clone(), body["secure"].clone())
        };
        let pooled = target("qd-lake-replication:9100");
        assert_eq!(pooled, ("qd-lake-replication:9100".into(), false.into()));
        assert_eq!(target("lake-1:9100").1, serde_json::Value::from(true));
    }
}
