//! The `maintain` job (decisions 0039 and 0061): a dataset's bronze and
//! silver tables kept small and clean. Per table, on DuckDB, its small files compacted and its
//! snapshots read; then, through the catalog and the bucket with the job's
//! credentials, the snapshots older than the retention expired (never the
//! current one) and the data files no kept snapshot refers to, older than
//! the orphans' retention, deleted. Only data files are touched; names and
//! counts come out, never a value.

use std::collections::HashSet;
use std::time::Duration;

use chrono::{DateTime, Utc};
use quality::job_maintain::{retention, MaintainReport, TableMaintained};
use quality::lake::LakeAccess;
use quality::layers;
use quality::worker_api::JobDetails;
use quality_rules_duckdb::{LakeConnection, Runner};
use sigv4::{encode, payload_hash, Request};

use crate::evaluate::{failed, Failed};

/// What DuckDB found of one table.
struct Seen {
    namespace: String,
    name: String,
    files_before: i64,
    files_after: i64,
    current: i64,
    expire: Vec<i64>,
    /// Data files the kept snapshots' manifests name, in any entry, as
    /// `s3://` paths: none of them is an orphan.
    referenced: HashSet<String>,
    /// `tables/<uuid>/data/`, from the files' paths.
    data_prefix: Option<String>,
}

fn engine(stage: &str) -> impl Fn(quality_rules_duckdb::Error) -> Failed + '_ {
    move |_| failed(format!("maintain: the engine failed while {stage}"))
}

/// The DuckDB part: compaction, snapshots and the files kept ones use.
fn inspect(
    job: &JobDetails,
    ca_file: Option<&str>,
    now: DateTime<Utc>,
) -> Result<Vec<Seen>, Failed> {
    let dataset = job
        .dataset
        .as_deref()
        .ok_or_else(|| failed("maintain: the job has no dataset"))?;
    let lake = job
        .lake
        .as_ref()
        .ok_or_else(|| failed("maintain: no lake access came with the job"))?;
    let (snapshot_days, _) = retention(&job.parameters);
    let cutoff = now.timestamp_millis() - snapshot_days * 86_400_000;
    let mut runner = Runner::new().map_err(engine("starting"))?;
    runner
        .attach_lake(&LakeConnection {
            endpoint: &lake.endpoint,
            warehouse: &lake.warehouse,
            access_key: &lake.access_key,
            secret_key: &lake.secret_key,
            session_token: &lake.session_token,
            ca_file,
        })
        .map_err(engine("attaching the lake"))?;
    let mut seen = Vec::new();
    for namespace in layers::namespaces(dataset) {
        for name in runner
            .lake_tables(&namespace)
            .map_err(engine("listing the tables"))?
        {
            if let Some(s) = inspect_table(&runner, lake, &namespace, name, cutoff)? {
                seen.push(s);
            }
        }
    }
    Ok(seen)
}

/// One table: compacted, its snapshots and the files kept ones use;
/// `None` if it has no snapshot.
fn inspect_table(
    runner: &Runner,
    lake: &LakeAccess,
    namespace: &str,
    name: String,
    cutoff: i64,
) -> Result<Option<Seen>, Failed> {
    let files_before = runner
        .data_files(namespace, &name, None)
        .map_err(engine("reading files"))?
        .len();
    runner
        .compact(namespace, &name)
        .map_err(engine("compacting"))?;
    let now_files = runner
        .data_files(namespace, &name, None)
        .map_err(engine("reading files"))?;
    let snapshots = runner
        .table_snapshots(namespace, &name)
        .map_err(engine("reading snapshots"))?;
    let Some(current) = snapshots.last().map(|s| s.id) else {
        return Ok(None);
    };
    let expire: Vec<i64> = snapshots
        .iter()
        .filter(|s| s.id != current && s.at_ms < cutoff)
        .map(|s| s.id)
        .collect();
    let mut referenced = HashSet::new();
    for kept in snapshots.iter().filter(|s| !expire.contains(&s.id)) {
        referenced.extend(
            runner
                .named_files(namespace, &name, kept.id)
                .map_err(engine("reading files"))?,
        );
    }
    let data_prefix = now_files.first().and_then(|p| {
        let rest = p.strip_prefix(&format!("s3://{}/", lake.warehouse))?;
        rest.find("/data/")
            .map(|at| rest[..at + "/data/".len()].to_string())
    });
    Ok(Some(Seen {
        namespace: namespace.to_string(),
        name,
        files_before: i64::try_from(files_before).unwrap_or(i64::MAX),
        files_after: i64::try_from(now_files.len()).unwrap_or(i64::MAX),
        current,
        expire,
        referenced,
        data_prefix,
    }))
}

/// A request to the lake signed with the job's credentials.
async fn send(
    http: &reqwest::Client,
    lake: &LakeAccess,
    method: &str,
    path: &str,
    query: &[(&str, &str)],
    body: Vec<u8>,
) -> Result<reqwest::Response, Failed> {
    let unreachable = |_| failed("maintain: the lake did not answer");
    let host =
        sigv4::host(&lake.endpoint).ok_or_else(|| failed("maintain: a bad lake endpoint"))?;
    let hash = payload_hash(&body);
    let headers = [
        ("content-type", "application/json"),
        ("host", host.as_str()),
        ("x-amz-content-sha256", hash.as_str()),
        ("x-amz-security-token", lake.session_token.as_str()),
    ];
    let request = Request {
        method,
        path,
        query,
        headers: &headers,
    };
    let signed = sigv4::sign_request(
        "s3",
        &lake.access_key,
        &lake.secret_key,
        &request,
        Utc::now(),
    );
    let q = sigv4::query_string(query);
    let url = format!(
        "{}{path}{}{q}",
        lake.endpoint.trim_end_matches('/'),
        if q.is_empty() { "" } else { "?" }
    );
    let method = reqwest::Method::from_bytes(method.as_bytes())
        .map_err(|_| failed("maintain: a bad method"))?;
    let mut req = http
        .request(method, &url)
        .body(body)
        .timeout(Duration::from_secs(60));
    for (name, value) in headers.iter().filter(|(n, _)| *n != "host") {
        req = req.header(*name, *value);
    }
    req.header("x-amz-date", &signed.date)
        .header("authorization", &signed.authorization)
        .send()
        .await
        .map_err(unreachable)
}

/// The keys and last modifications under `prefix`, all pages, from the
/// bucket's `ListObjectsV2` answer.
async fn listed(
    http: &reqwest::Client,
    lake: &LakeAccess,
    prefix: &str,
) -> Result<Vec<(String, String)>, Failed> {
    let path = format!("/{}", encode(&lake.warehouse, false));
    let mut out = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut query = vec![("list-type", "2"), ("prefix", prefix)];
        if let Some(t) = &token {
            query.push(("continuation-token", t.as_str()));
        }
        let res = send(http, lake, "GET", &path, &query, Vec::new()).await?;
        if !res.status().is_success() {
            return Err(failed(format!(
                "maintain: listing refused ({})",
                res.status().as_u16()
            )));
        }
        let xml = res
            .text()
            .await
            .map_err(|_| failed("maintain: a bad listing"))?;
        for contents in xml.split("<Contents>").skip(1) {
            if let (Some(key), Some(at)) = (tag(contents, "Key"), tag(contents, "LastModified")) {
                out.push((key.replace("&amp;", "&"), at.to_string()));
            }
        }
        token = (tag(&xml, "IsTruncated") == Some("true"))
            .then(|| tag(&xml, "NextContinuationToken").map(str::to_string))
            .flatten();
        if token.is_none() {
            return Ok(out);
        }
    }
}

fn tag<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let start = xml.find(&format!("<{name}>"))? + name.len() + 2;
    let end = xml[start..].find(&format!("</{name}>"))? + start;
    Some(&xml[start..end])
}

/// Runs the job: DuckDB's part off the runtime, then the catalog's and
/// the bucket's.
pub(crate) async fn maintain(
    http: &reqwest::Client,
    job: JobDetails,
    ca: Option<String>,
) -> Result<MaintainReport, Failed> {
    let now = Utc::now();
    let (snapshot_days, orphan_days) = retention(&job.parameters);
    let job = std::sync::Arc::new(job);
    let j = job.clone();
    let seen = crate::job_span::blocking(move || inspect(&j, ca.as_deref(), now))
        .await
        .map_err(|_| failed("maintain: the engine stopped"))??;
    let lake = job
        .lake
        .as_ref()
        .ok_or_else(|| failed("maintain: no lake access came with the job"))?;
    let orphan_cutoff = now - chrono::Duration::days(orphan_days);
    let mut tables = Vec::new();
    for t in seen {
        let mut expired = 0;
        if !t.expire.is_empty() {
            let path = format!(
                "/iceberg/v1/{}/namespaces/{}/tables/{}",
                encode(&lake.warehouse, false),
                encode(&t.namespace, false),
                encode(&t.name, false)
            );
            let body = serde_json::json!({
                "requirements": [{ "type": "assert-ref-snapshot-id", "ref": "main", "snapshot-id": t.current }],
                "updates": [{ "action": "remove-snapshots", "snapshot-ids": t.expire }],
            });
            let res = send(
                http,
                lake,
                "POST",
                &path,
                &[],
                body.to_string().into_bytes(),
            )
            .await?;
            // Another commit since: the table is left for the next run.
            if res.status().is_success() {
                expired = t.expire.len();
            }
        }
        let mut deleted = 0;
        if let Some(prefix) = &t.data_prefix {
            for (key, at) in listed(http, lake, prefix).await? {
                let old = DateTime::parse_from_rfc3339(&at).is_ok_and(|at| at < orphan_cutoff);
                let used = t
                    .referenced
                    .contains(&format!("s3://{}/{key}", lake.warehouse));
                if old && !used {
                    let path =
                        format!("/{}/{}", encode(&lake.warehouse, false), encode(&key, true));
                    let res = send(http, lake, "DELETE", &path, &[], Vec::new()).await?;
                    if res.status().is_success() {
                        deleted += 1;
                    }
                }
            }
        }
        tables.push(TableMaintained {
            namespace: t.namespace,
            table: t.name,
            files_before: t.files_before,
            files_after: t.files_after,
            snapshots_expired: i64::try_from(expired).unwrap_or(i64::MAX),
            orphans_deleted: deleted,
        });
    }
    Ok(MaintainReport {
        snapshot_days,
        orphan_days,
        tables,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listings_keys_dates_and_next_page_are_read_from_its_xml() {
        let xml = "<ListBucketResult><IsTruncated>true</IsTruncated>\
            <Contents><Key>tables/u/data/a&amp;b.parquet</Key><LastModified>2026-10-01T00:00:00.000Z</LastModified></Contents>\
            <NextContinuationToken>t1</NextContinuationToken></ListBucketResult>";
        assert_eq!(tag(xml, "IsTruncated"), Some("true"));
        assert_eq!(tag(xml, "NextContinuationToken"), Some("t1"));
        let contents = xml.split("<Contents>").nth(1).unwrap();
        assert_eq!(tag(contents, "Key"), Some("tables/u/data/a&amp;b.parquet"));
        assert!(DateTime::parse_from_rfc3339(tag(contents, "LastModified").unwrap()).is_ok());
    }
}
