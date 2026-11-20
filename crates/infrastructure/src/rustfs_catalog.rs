//! A table's place in its warehouse, from the lake's Iceberg REST catalog
//! (decision 0037): `GET /iceberg/v1/<warehouse>/namespaces/<ns>/tables/<t>`,
//! signed with credentials issued for that read by STS and limited by the
//! caller's policy. Only the table's location is read; RustFS vends no
//! credentials in its answer (`rustfs.credential-vending: unsupported`).

use std::time::Duration;

use chrono::Utc;

use crate::rustfs_sts::{StsIssuer, TemporaryCredentials};
use sigv4::{encode, payload_hash};

/// Looks tables up through the platform's STS issuer.
#[derive(Debug, Clone)]
pub struct TableLookup {
    issuer: StsIssuer,
}

impl TableLookup {
    pub fn new(issuer: StsIssuer) -> Self {
        Self { issuer }
    }

    /// The table's prefix in the bucket (`tables/<uuid>`), `None` if the
    /// catalog has no such table. `policy` is the session policy for it.
    pub async fn prefix(
        &self,
        warehouse: &str,
        namespace: &str,
        table: &str,
        policy: &str,
    ) -> Result<Option<String>, String> {
        let path = format!(
            "/iceberg/v1/{}/namespaces/{}/tables/{}",
            encode(warehouse, false),
            encode(namespace, false),
            encode(table, false)
        );
        let res = self.send("GET", &path, &[], Vec::new(), policy).await?;
        match res.status().as_u16() {
            200 => {}
            404 => return Ok(None),
            status => return Err(format!("catalog refused ({status})")),
        }
        let body: serde_json::Value = res.json().await.map_err(|e| format!("catalog: {e}"))?;
        let location = body["metadata"]["location"]
            .as_str()
            .ok_or("catalog: no location")?;
        prefix_of(warehouse, location)
            .map(Some)
            .ok_or_else(|| "catalog: a location outside the warehouse".to_string())
    }

    /// The tables of `namespace`, by name, all pages; none if it is not
    /// there.
    pub async fn tables(
        &self,
        warehouse: &str,
        namespace: &str,
        policy: &str,
    ) -> Result<Vec<String>, String> {
        let base = format!(
            "/iceberg/v1/{}/namespaces/{}/tables",
            encode(warehouse, false),
            encode(namespace, false)
        );
        let mut names = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let query: Vec<(&str, &str)> = match &token {
                Some(t) => vec![("pageToken", t.as_str())],
                None => Vec::new(),
            };
            let res = self.send("GET", &base, &query, Vec::new(), policy).await?;
            match res.status().as_u16() {
                200 => {}
                404 => return Ok(names),
                status => return Err(format!("catalog refused the list ({status})")),
            }
            let body: serde_json::Value = res.json().await.map_err(|e| format!("catalog: {e}"))?;
            if let Some(ids) = body["identifiers"].as_array() {
                names.extend(
                    ids.iter()
                        .filter_map(|i| i["name"].as_str().map(str::to_string)),
                );
            }
            match body["next-page-token"].as_str().filter(|t| !t.is_empty()) {
                Some(t) => token = Some(t.to_string()),
                None => return Ok(names),
            }
        }
    }

    /// Creates the namespace unless it is there (409 is fine): the
    /// application's part, so a job needs no right to create namespaces,
    /// which RustFS cannot narrow to one (decision 0037).
    pub async fn ensure_namespace(
        &self,
        warehouse: &str,
        namespace: &str,
        policy: &str,
    ) -> Result<(), String> {
        let path = format!("/iceberg/v1/{}/namespaces", encode(warehouse, false));
        let body = serde_json::to_vec(&serde_json::json!({
            "namespace": [namespace],
            "properties": {},
        }))
        .map_err(|e| e.to_string())?;
        let res = self.send("POST", &path, &[], body, policy).await?;
        match res.status().as_u16() {
            200 | 409 => Ok(()),
            status => Err(format!("catalog refused the namespace ({status})")),
        }
    }

    /// `method path?query` with `body`, signed with credentials issued
    /// within `policy` for this call.
    async fn send(
        &self,
        method: &str,
        path: &str,
        query: &[(&str, &str)],
        body: Vec<u8>,
        policy: &str,
    ) -> Result<reqwest::Response, String> {
        let creds = self.credentials("catalog", policy).await?;
        self.send_as(&creds, method, path, query, body).await
    }

    /// Credentials issued within `policy`, for the calls of one task.
    pub(crate) async fn credentials(
        &self,
        session: &str,
        policy: &str,
    ) -> Result<TemporaryCredentials, String> {
        self.issuer
            .assume_role(session, policy, Duration::from_secs(900))
            .await
            .map_err(|e| e.to_string())
    }

    /// `method path?query` with `body`, signed with `creds`.
    pub(crate) async fn send_as(
        &self,
        creds: &TemporaryCredentials,
        method: &str,
        path: &str,
        query: &[(&str, &str)],
        body: Vec<u8>,
    ) -> Result<reqwest::Response, String> {
        let query_string = sigv4::query_string(query);
        let url = if query_string.is_empty() {
            format!("{}{path}", self.issuer.endpoint())
        } else {
            format!("{}{path}?{query_string}", self.issuer.endpoint())
        };
        let host = sigv4::host(&url).ok_or_else(|| format!("bad endpoint {url}"))?;
        let hash = payload_hash(&body);
        let headers = [
            ("content-type", "application/json"),
            ("host", host.as_str()),
            ("x-amz-content-sha256", hash.as_str()),
            ("x-amz-security-token", creds.session_token.as_str()),
        ];
        let request = sigv4::Request {
            method,
            path,
            query,
            headers: &headers,
        };
        let signed = sigv4::sign_request(
            "s3",
            &creds.access_key,
            &creds.secret_key,
            &request,
            Utc::now(),
        );
        let method = reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| e.to_string())?;
        let mut req = self.issuer.http().request(method, &url).body(body);
        for (name, value) in headers.iter().filter(|(n, _)| *n != "host") {
            req = req.header(*name, *value);
        }
        req.header("x-amz-date", &signed.date)
            .header("authorization", &signed.authorization)
            .send()
            .await
            .map_err(|e| format!("catalog: {e}"))
    }
}

/// `tables/<uuid>` of `s3://<warehouse>/tables/<uuid>`, if it is there.
fn prefix_of(warehouse: &str, location: &str) -> Option<String> {
    let prefix = location
        .strip_prefix(&format!("s3://{warehouse}/"))?
        .trim_end_matches('/');
    let valid = !prefix.is_empty()
        && !prefix
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
        && !prefix.contains('*');
    valid.then(|| prefix.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tables_prefix_is_inside_its_warehouse_or_none() {
        assert_eq!(
            prefix_of("qd-w", "s3://qd-w/tables/7bc3d843").as_deref(),
            Some("tables/7bc3d843")
        );
        for outside in [
            "s3://qd-other/tables/7bc3d843",
            "s3://qd-w/",
            "s3://qd-w/tables/../x",
            "s3://qd-w/tables/*",
            "file:///tmp/t",
        ] {
            assert_eq!(prefix_of("qd-w", outside), None, "{outside}");
        }
    }
}
