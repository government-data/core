//! The data protocols at the wire, S3, Iceberg REST and STS (decision 0078,
//! point 6), with standard requests signed by SigV4. They need a lake that
//! speaks them, so they run against RustFS only (step 14, the user's ruling
//! of 2026-11-23), never against the in-memory fake.

use application::lake::{Lake, LakeCredentials, LakeGrant, LakeProperties, UrlStyle};
use serde_json::{json, Value};

mod avro;
use avro::empty_manifest_list;

/// Keys a request is signed with: the lake's own, or a job's from STS.
#[derive(Clone)]
pub struct Keys {
    pub access_key: String,
    pub secret_key: String,
    pub session_token: Option<String>,
}

impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Keys(…)")
    }
}

impl From<&LakeCredentials> for Keys {
    fn from(c: &LakeCredentials) -> Self {
        Self {
            access_key: c.access_key.clone(),
            secret_key: c.secret_key.clone(),
            session_token: Some(c.session_token.clone()),
        }
    }
}

/// A caller of a lake's S3 and Iceberg REST catalog, as its properties say
/// where they are.
#[derive(Debug, Clone)]
pub struct DataClient {
    properties: LakeProperties,
    keys: Keys,
    http: reqwest::Client,
}

/// An answer: its status and its body.
pub type Answer = (u16, Vec<u8>);

impl DataClient {
    /// `http` must already trust the lake's certificate (and present one
    /// at a gate that asks for it).
    pub fn new(properties: LakeProperties, keys: Keys, http: reqwest::Client) -> Self {
        Self {
            properties,
            keys,
            http,
        }
    }

    /// The same lake, with other keys.
    pub fn with_keys(&self, keys: Keys) -> Self {
        Self {
            keys,
            ..self.clone()
        }
    }

    /// An S3 request on `key` of `bucket` (an empty key: the bucket).
    pub async fn s3(
        &self,
        method: &str,
        bucket: &str,
        key: &str,
        body: &[u8],
        extra: &[(&str, &str)],
    ) -> Answer {
        let base = self.properties.s3_endpoint.trim_end_matches('/');
        let url = match self.properties.url_style {
            UrlStyle::Path => format!("{base}/{bucket}/{key}"),
            UrlStyle::VirtualHosted => {
                let (scheme, host) = base.split_once("://").expect("an absolute URL");
                format!("{scheme}://{bucket}.{host}/{key}")
            }
        };
        self.signed(method, &url, "s3", body, extra).await
    }

    /// A catalog request on `path` under `/v1/` (`config`, or `<prefix>/…`).
    pub async fn catalog(&self, method: &str, path: &str, body: Option<&Value>) -> Answer {
        let url = format!(
            "{}/v1/{path}",
            self.properties.catalog_uri.trim_end_matches('/')
        );
        let bytes = body.map(|b| b.to_string().into_bytes()).unwrap_or_default();
        let service = self.properties.catalog_sigv4_service.clone();
        let extra: &[(&str, &str)] = if body.is_some() {
            &[("content-type", "application/json")]
        } else {
            &[]
        };
        self.signed(method, &url, &service, &bytes, extra).await
    }

    async fn signed(
        &self,
        method: &str,
        url: &str,
        service: &str,
        body: &[u8],
        extra: &[(&str, &str)],
    ) -> Answer {
        let host = sigv4::host(url).expect("a host");
        let (path, query) = split_url(url);
        let hash = sigv4::payload_hash(body);
        let mut headers: Vec<(&str, &str)> = vec![("host", &host), ("x-amz-content-sha256", &hash)];
        headers.extend_from_slice(extra);
        if let Some(token) = &self.keys.session_token {
            headers.push(("x-amz-security-token", token));
        }
        let query_pairs: Vec<(&str, &str)> = query
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let request = sigv4::Request {
            method,
            path: &path,
            query: &query_pairs,
            headers: &headers,
        };
        let signed = sigv4::sign_request(
            service,
            &self.keys.access_key,
            &self.keys.secret_key,
            &request,
            chrono::Utc::now(),
        );
        let method = reqwest::Method::from_bytes(method.as_bytes()).expect("a method");
        let mut builder = self.http.request(method, url).body(body.to_vec());
        for (name, value) in headers.iter().filter(|(n, _)| *n != "host") {
            builder = builder.header(*name, *value);
        }
        let response = builder
            .header("x-amz-date", &signed.date)
            .header("authorization", &signed.authorization)
            .send()
            .await
            .expect("the lake answers");
        let status = response.status().as_u16();
        (status, response.bytes().await.expect("its body").to_vec())
    }

    /// The catalog's prefix for `warehouse`, from `/v1/config` as RustFS
    /// gives it (`defaults.prefix`, an override winning).
    pub async fn prefix(&self, warehouse: &str) -> String {
        let (status, body) = self
            .catalog("GET", &format!("config?warehouse={warehouse}"), None)
            .await;
        assert_eq!(status, 200, "GET /v1/config");
        let config: Value = serde_json::from_slice(&body).expect("a config");
        config["overrides"]["prefix"]
            .as_str()
            .or_else(|| config["defaults"]["prefix"].as_str())
            .expect("a prefix")
            .to_string()
    }
}

/// The path and the query of `url`, the query decoded into pairs.
fn split_url(url: &str) -> (String, Vec<(String, String)>) {
    let after_host = url.splitn(4, '/').nth(3).unwrap_or_default();
    let (path, query) = after_host.split_once('?').unwrap_or((after_host, ""));
    let pairs = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (k.to_string(), v.to_string())
        })
        .collect();
    (format!("/{path}"), pairs)
}

/// An object written once: the second write refused (`412`), the first
/// body kept.
pub async fn put_once(client: &DataClient, warehouse: &str) {
    let key = "contract/put-once.txt";
    let once = [("if-none-match", "*")];
    let (first, _) = client.s3("PUT", warehouse, key, b"first", &once).await;
    let (second, _) = client.s3("PUT", warehouse, key, b"second", &once).await;
    assert_eq!(
        (first, second),
        (200, 412),
        "If-None-Match: * on a second write"
    );
    let (status, body) = client.s3("GET", warehouse, key, b"", &[]).await;
    assert_eq!((status, body.as_slice()), (200, b"first".as_slice()));
}

/// A job's credentials from STS within a read-only grant: they read the
/// warehouse, and are refused a write there and a read of another one.
pub async fn session_policy_holds(
    lake: &dyn Lake,
    client: &DataClient,
    warehouse: &str,
    other: &str,
) {
    let grant = LakeGrant::read_only(warehouse);
    let minimum = lake.properties().sts_min_duration;
    let issued = lake
        .credentials(&grant, "contract", minimum)
        .await
        .expect("credentials");
    let job = client.with_keys(Keys::from(&issued));
    let (read, _) = job.s3("GET", warehouse, "", b"", &[]).await;
    assert_eq!(read, 200, "the grant reads its warehouse");
    let (write, _) = job
        .s3("PUT", warehouse, "contract/denied.txt", b"x", &[])
        .await;
    assert_eq!(write, 403, "a read-only grant writes nothing");
    let (elsewhere, _) = job.s3("GET", other, "", b"", &[]).await;
    assert_eq!(elsewhere, 403, "nothing outside its warehouse");
}

/// A namespace made twice (`409` the second time), a table created,
/// committed, listed, its earlier metadata still readable (time travel by
/// the metadata log), and dropped (files first where a drop does not purge
/// them).
pub async fn table_lifecycle(client: &DataClient, warehouse: &str) {
    let prefix = client.prefix(warehouse).await;
    let ns = "contract_ns";
    let make = json!({ "namespace": [ns], "properties": {} });
    let (first, _) = client
        .catalog("POST", &format!("{prefix}/namespaces"), Some(&make))
        .await;
    let (second, _) = client
        .catalog("POST", &format!("{prefix}/namespaces"), Some(&make))
        .await;
    assert_eq!((first, second), (200, 409), "a namespace made twice");

    let table = json!({ "name": "t", "schema": { "type": "struct", "schema-id": 0, "fields": [
        { "id": 1, "name": "n", "required": false, "type": "long" } ] } });
    let tables = format!("{prefix}/namespaces/{ns}/tables");
    let (status, body) = client.catalog("POST", &tables, Some(&table)).await;
    assert_eq!(
        status,
        200,
        "a table created: {}",
        String::from_utf8_lossy(&body)
    );
    let created: Value = serde_json::from_slice(&body).expect("a table");
    let uuid = created["metadata"]["table-uuid"]
        .as_str()
        .expect("its uuid")
        .to_string();
    let first_metadata = created["metadata-location"]
        .as_str()
        .expect("where")
        .to_string();

    let table_url = format!("{tables}/t");
    let location = created["metadata"]["location"]
        .as_str()
        .expect("its location")
        .to_string();
    commit_two_snapshots(client, &table_url, &uuid, &location).await;

    listed_and_travelled(client, &tables, &table_url, &first_metadata).await;
    drop_table(client, &tables, &location).await;
}

/// Two snapshots committed one after the other, each with its manifest
/// list written first under the table's location (an Avro container with
/// no manifest: the snapshot adds no data file), and a commit whose requirement
/// no longer holds refused.
async fn commit_two_snapshots(client: &DataClient, table_url: &str, uuid: &str, location: &str) {
    let (bucket, prefix) = location
        .strip_prefix("s3://")
        .and_then(|s| s.split_once('/'))
        .expect("an s3 location");
    for (id, parent) in [(1_i64, None), (2, Some(1_i64))] {
        let key = format!("{prefix}/metadata/snap-{id}.avro");
        let (status, _) = client
            .s3("PUT", bucket, &key, &empty_manifest_list(), &[])
            .await;
        assert_eq!(status, 200, "a manifest list written");
        let mut snapshot = json!({
            "snapshot-id": id,
            "sequence-number": id,
            "timestamp-ms": 1_700_000_000_000_i64 + id,
            "manifest-list": format!("s3://{bucket}/{key}"),
            "summary": { "operation": "append" },
            "schema-id": 0,
        });
        if let Some(parent) = parent {
            snapshot["parent-snapshot-id"] = json!(parent);
        }
        let commit = json!({
            "requirements": [
                { "type": "assert-table-uuid", "uuid": uuid },
                { "type": "assert-ref-snapshot-id", "ref": "main", "snapshot-id": parent },
            ],
            "updates": [
                { "action": "add-snapshot", "snapshot": snapshot },
                { "action": "set-snapshot-ref", "ref-name": "main", "type": "branch", "snapshot-id": id },
            ],
        });
        let (status, body) = client.catalog("POST", table_url, Some(&commit)).await;
        assert_eq!(
            status,
            200,
            "snapshot {id} committed: {}",
            String::from_utf8_lossy(&body)
        );
    }
    let stale = json!({
        "requirements": [{ "type": "assert-ref-snapshot-id", "ref": "main", "snapshot-id": 1 }],
        "updates": [{ "action": "set-properties", "updates": { "contract": "stale" } }],
    });
    let (status, _) = client.catalog("POST", table_url, Some(&stale)).await;
    assert_eq!(
        status, 409,
        "a commit whose requirement no longer holds is refused"
    );
}

/// The table listed, its metadata log keeping the version before, and
/// both snapshots kept with the latest current: the earlier one can be
/// read (time travel).
async fn listed_and_travelled(
    client: &DataClient,
    tables: &str,
    table_url: &str,
    first_metadata: &str,
) {
    let (status, body) = client.catalog("GET", tables, None).await;
    assert_eq!(status, 200);
    let listed: Value = serde_json::from_slice(&body).expect("a list");
    assert!(
        listed.to_string().contains("\"t\""),
        "the table is listed: {listed}"
    );

    let (_, body) = client.catalog("GET", table_url, None).await;
    let loaded: Value = serde_json::from_slice(&body).expect("a table");
    let log = loaded["metadata"]["metadata-log"].to_string();
    assert!(
        log.contains(first_metadata),
        "the metadata log keeps the earlier version"
    );
    // Time travel: the earlier snapshot is still the table's, in its log.
    let snapshots = loaded["metadata"]["snapshots"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let ids: Vec<i64> = snapshots
        .iter()
        .filter_map(|s| s["snapshot-id"].as_i64())
        .collect();
    assert_eq!(ids, [1, 2], "both snapshots kept");
    let current = loaded["metadata"]["current-snapshot-id"].as_i64();
    assert_eq!(current, Some(2), "the latest is current");
    let snapshot_log = loaded["metadata"]["snapshot-log"].to_string();
    assert!(
        snapshot_log.contains("\"snapshot-id\":1"),
        "the earlier one in the log"
    );
}

async fn drop_table(client: &DataClient, tables: &str, location: &str) {
    let (bucket, prefix) = location
        .strip_prefix("s3://")
        .and_then(|s| s.split_once('/'))
        .expect("an s3 location");
    if !client.properties.drop_purges_files {
        let list_key = format!("?list-type=2&prefix={prefix}/");
        let (status, body) = client.s3("GET", bucket, &list_key, b"", &[]).await;
        assert_eq!(status, 200, "the table's files listed");
        let text = String::from_utf8_lossy(&body).to_string();
        for key in text
            .split("<Key>")
            .skip(1)
            .filter_map(|s| s.split("</Key>").next())
        {
            let (status, _) = client.s3("DELETE", bucket, key, b"", &[]).await;
            assert_eq!(status, 204, "a file of the table deleted");
        }
    }
    let purge = if client.properties.drop_purges_files {
        "?purgeRequested=true"
    } else {
        ""
    };
    let (status, _) = client
        .catalog("DELETE", &format!("{tables}/t{purge}"), None)
        .await;
    assert_eq!(status, 204, "dropped");
    let (status, _) = client.catalog("GET", &format!("{tables}/t"), None).await;
    assert_eq!(status, 404, "a dropped table is gone");
}
