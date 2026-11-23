//! A table's place in its warehouse, from the lake's Iceberg REST catalog
//! (decisions 0037 and 0078): `GET <catalog>/v1/<prefix>/namespaces/<ns>/tables/<t>`,
//! where the lake declares its catalog and prefix, signed with credentials
//! issued for that read by STS and limited by the caller's policy. Only
//! the table's location is read: the core asks for no vended credentials.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use application::lake::CatalogPrefix;
use chrono::Utc;

use crate::lake_sts::{StsIssuer, TemporaryCredentials};
use sigv4::{encode, payload_hash};

/// Looks tables up through the platform's STS issuer.
#[derive(Debug, Clone)]
pub struct TableLookup {
    issuer: StsIssuer,
    /// Each warehouse's prefix, once `/v1/config` gave it.
    prefixes: Arc<Mutex<HashMap<String, String>>>,
}

impl TableLookup {
    pub fn new(issuer: StsIssuer) -> Self {
        Self {
            issuer,
            prefixes: Arc::default(),
        }
    }

    /// The issuer, for the files of a table (the lake's objects).
    pub(crate) fn issuer(&self) -> &StsIssuer {
        &self.issuer
    }

    /// `<catalog>/v1/<prefix>` for `warehouse`: the declared prefix, or the
    /// one `/v1/config?warehouse=` gives, asked once with `creds`.
    pub(crate) async fn catalog_base(
        &self,
        creds: &TemporaryCredentials,
        warehouse: &str,
    ) -> Result<String, String> {
        let catalog = self
            .issuer
            .properties()
            .catalog_uri
            .trim_end_matches('/')
            .to_string();
        let prefix = match &self.issuer.properties().catalog_prefix {
            CatalogPrefix::Fixed(p) => p.clone(),
            CatalogPrefix::FromConfig => {
                let known = self
                    .prefixes
                    .lock()
                    .map_err(|e| e.to_string())?
                    .get(warehouse)
                    .cloned();
                match known {
                    Some(p) => p,
                    None => self.prefix_from_config(creds, &catalog, warehouse).await?,
                }
            }
        };
        Ok(format!("{catalog}/v1/{}", encode(&prefix, false)))
    }

    async fn prefix_from_config(
        &self,
        creds: &TemporaryCredentials,
        catalog: &str,
        warehouse: &str,
    ) -> Result<String, String> {
        let url = format!("{catalog}/v1/config");
        let query = [("warehouse", warehouse)];
        let res = self
            .send_as(creds, "GET", &url, &query, Vec::new(), true)
            .await?;
        if res.status().as_u16() != 200 {
            return Err(format!(
                "catalog config refused ({})",
                res.status().as_u16()
            ));
        }
        let config: serde_json::Value = res.json().await.map_err(|e| format!("catalog: {e}"))?;
        let prefix = config["overrides"]["prefix"]
            .as_str()
            .or_else(|| config["defaults"]["prefix"].as_str())
            .ok_or("catalog config: no prefix")?
            .to_string();
        self.prefixes
            .lock()
            .map_err(|e| e.to_string())?
            .insert(warehouse.to_string(), prefix.clone());
        Ok(prefix)
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
        let creds = self.credentials("catalog", policy).await?;
        let base = self.catalog_base(&creds, warehouse).await?;
        let url = format!(
            "{base}/namespaces/{}/tables/{}",
            encode(namespace, false),
            encode(table, false)
        );
        let res = self
            .send_as(&creds, "GET", &url, &[], Vec::new(), true)
            .await?;
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
        let creds = self.credentials("catalog", policy).await?;
        let base = format!(
            "{}/namespaces/{}/tables",
            self.catalog_base(&creds, warehouse).await?,
            encode(namespace, false)
        );
        let mut names = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let query: Vec<(&str, &str)> = match &token {
                Some(t) => vec![("pageToken", t.as_str())],
                None => Vec::new(),
            };
            let res = self
                .send_as(&creds, "GET", &base, &query, Vec::new(), true)
                .await?;
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
    /// which a lake may not narrow to one (decision 0037).
    pub async fn ensure_namespace(
        &self,
        warehouse: &str,
        namespace: &str,
        policy: &str,
    ) -> Result<(), String> {
        let creds = self.credentials("catalog", policy).await?;
        let url = format!("{}/namespaces", self.catalog_base(&creds, warehouse).await?);
        let body = serde_json::to_vec(&serde_json::json!({
            "namespace": [namespace],
            "properties": {},
        }))
        .map_err(|e| e.to_string())?;
        let res = self.send_as(&creds, "POST", &url, &[], body, true).await?;
        match res.status().as_u16() {
            200 | 409 => Ok(()),
            status => Err(format!("catalog refused the namespace ({status})")),
        }
    }

    /// Credentials issued within `policy`, for the calls of one task.
    pub(crate) async fn credentials(
        &self,
        session: &str,
        policy: &str,
    ) -> Result<TemporaryCredentials, String> {
        self.issuer
            .assume_role(session, policy, Duration::from_secs(0))
            .await
            .map_err(|e| e.to_string())
    }

    /// `method url?query` with `body`, signed with `creds`: for the
    /// catalog's service if `catalog`, else S3's.
    pub(crate) async fn send_as(
        &self,
        creds: &TemporaryCredentials,
        method: &str,
        url: &str,
        query: &[(&str, &str)],
        body: Vec<u8>,
        catalog: bool,
    ) -> Result<reqwest::Response, String> {
        let path = path_of(url).ok_or_else(|| format!("bad URL {url}"))?;
        let query_string = sigv4::query_string(query);
        let url = if query_string.is_empty() {
            url.to_string()
        } else {
            format!("{url}?{query_string}")
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
            path: &path,
            query,
            headers: &headers,
        };
        let service = if catalog {
            self.issuer.properties().catalog_sigv4_service.as_str()
        } else {
            "s3"
        };
        let signed = sigv4::sign_request(
            service,
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

/// The path of an absolute URL, as it is signed (`/` when it has none).
fn path_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    Some(match rest.find('/') {
        Some(i) => rest[i..].to_string(),
        None => "/".to_string(),
    })
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
    fn a_urls_path_is_what_is_signed() {
        assert_eq!(
            path_of("https://lake.test/iceberg/v1/w").as_deref(),
            Some("/iceberg/v1/w")
        );
        assert_eq!(path_of("https://lake.test").as_deref(), Some("/"));
        assert_eq!(path_of("lake.test/x"), None);
    }

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
