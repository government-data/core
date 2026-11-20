//! Deleting a table of the lake (decision 0055), as RustFS lets it be done
//! (`tools/verify/lake_drop.py`): with one set of credentials from STS,
//! narrowed by the caller's policy to that table, its files first, while
//! the catalog still names it (RustFS refuses them after), then the table
//! from the catalog (RustFS takes no `purgeRequested`), then its namespace
//! if it is left empty. Run again, it finds no file and drops what is left.

use sigv4::encode;

use crate::rustfs_catalog::TableLookup;

/// The keys of a `ListObjectsV2` page, and the token of the next one.
fn listed(xml: &str) -> (Vec<String>, Option<String>) {
    let between = |s: &str, open: &str, close: &str| -> Vec<String> {
        s.split(open)
            .skip(1)
            .filter_map(|p| p.split(close).next())
            .map(|v| {
                v.replace("&amp;", "&")
                    .replace("&lt;", "<")
                    .replace("&gt;", ">")
            })
            .collect()
    };
    let keys = between(xml, "<Key>", "</Key>");
    let truncated = xml.contains("<IsTruncated>true</IsTruncated>");
    let next = between(xml, "<NextContinuationToken>", "</NextContinuationToken>")
        .into_iter()
        .next()
        .filter(|_| truncated);
    (keys, next)
}

impl TableLookup {
    /// Deletes `namespace.table`, whose files are under `prefix`
    /// (`tables/<uuid>`), with credentials issued within `policy`; then the
    /// namespace, if nothing is left in it.
    pub async fn drop(
        &self,
        warehouse: &str,
        (namespace, table): (&str, &str),
        prefix: &str,
        policy: &str,
    ) -> Result<(), String> {
        let creds = self.credentials("drop", policy).await?;
        let bucket = format!("/{}", encode(warehouse, false));
        let listing = format!("{}/", prefix.trim_end_matches('/'));
        let mut token: Option<String> = None;
        loop {
            let mut query = vec![("list-type", "2"), ("prefix", listing.as_str())];
            if let Some(t) = &token {
                query.push(("continuation-token", t.as_str()));
            }
            let res = self
                .send_as(&creds, "GET", &bucket, &query, Vec::new())
                .await?;
            if res.status().as_u16() != 200 {
                return Err(format!(
                    "lake refused the listing ({})",
                    res.status().as_u16()
                ));
            }
            let xml = res.text().await.map_err(|e| format!("lake: {e}"))?;
            let (keys, next) = listed(&xml);
            for key in keys.iter().filter(|k| k.starts_with(&listing)) {
                let path = format!("{bucket}/{}", encode(key, true));
                let res = self
                    .send_as(&creds, "DELETE", &path, &[], Vec::new())
                    .await?;
                if !matches!(res.status().as_u16(), 200 | 204 | 404) {
                    return Err(format!("lake refused a file ({})", res.status().as_u16()));
                }
            }
            match next {
                Some(t) => token = Some(t),
                None => break,
            }
        }
        let ns = format!(
            "/iceberg/v1/{}/namespaces/{}",
            encode(warehouse, false),
            encode(namespace, false)
        );
        let path = format!("{ns}/tables/{}", encode(table, false));
        let res = self
            .send_as(&creds, "DELETE", &path, &[], Vec::new())
            .await?;
        if !matches!(res.status().as_u16(), 200 | 204 | 404) {
            return Err(format!(
                "catalog refused the drop ({})",
                res.status().as_u16()
            ));
        }
        // Another table still in it: the namespace stays (409).
        let res = self.send_as(&creds, "DELETE", &ns, &[], Vec::new()).await?;
        match res.status().as_u16() {
            200 | 204 | 404 | 409 => Ok(()),
            status => Err(format!("catalog refused the namespace ({status})")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listing_gives_its_keys_and_its_next_page() {
        let xml = "<ListBucketResult><IsTruncated>true</IsTruncated>\
            <Contents><Key>tables/a/data/x&amp;y.parquet</Key></Contents>\
            <Contents><Key>tables/a/metadata/v1.json</Key></Contents>\
            <NextContinuationToken>t2</NextContinuationToken></ListBucketResult>";
        let (keys, next) = listed(xml);
        assert_eq!(
            keys,
            ["tables/a/data/x&y.parquet", "tables/a/metadata/v1.json"]
        );
        assert_eq!(next.as_deref(), Some("t2"));
        let (keys, next) =
            listed("<ListBucketResult><IsTruncated>false</IsTruncated></ListBucketResult>");
        assert!(keys.is_empty() && next.is_none());
    }
}
