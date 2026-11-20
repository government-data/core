//! The session policies a job's lake credentials are issued within
//! (decisions 0017, 0030, 0037, 0039 and 0055), one per kind of job, each
//! narrowed to what the job touches. RustFS matches the catalog's actions
//! (`admin:*Table*`) only against the whole warehouse
//! (`tools/verify/lake_drop.py`, `tools/verify/lake_silver.py`): what
//! confines a job to its tables is its S3 statement.

use serde_json::json;
/// Writing objects under `manifests/` of the warehouse, nothing else: not
/// even reading them back (decision 0035).
pub fn manifests_policy(warehouse: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": ["s3:PutObject"],
            "Resource": [format!("arn:aws:s3:::{warehouse}/manifests/*")],
        }],
    })
    .to_string()
}

/// Reading the warehouse's objects and its tables' metadata, nothing else.
pub fn read_only_policy(warehouse: &str) -> String {
    let resources = [
        format!("arn:aws:s3:::{warehouse}"),
        format!("arn:aws:s3:::{warehouse}/*"),
    ];
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Effect": "Allow",
                "Action": ["s3:GetBucketLocation", "s3:ListBucket", "s3:GetObject"],
                "Resource": resources,
            },
            {
                "Effect": "Allow",
                "Action": [
                    "admin:GetTableCatalog", "admin:GetTableBucket", "admin:GetTableNamespace",
                    "admin:GetTable", "admin:GetTableMetadata", "admin:GetTableMetadataLocation",
                ],
                "Resource": resources,
            },
        ],
    })
    .to_string()
}

/// A listing's policy (decision 0043, point 1): the catalog and the
/// tables' metadata files, no data file, so it reads no row
/// (`tools/verify/lake_listing.py`).
pub fn metadata_policy(warehouse: &str) -> String {
    let bucket = format!("arn:aws:s3:::{warehouse}");
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Effect": "Allow",
                "Action": ["s3:GetBucketLocation", "s3:ListBucket"],
                "Resource": [&bucket],
            },
            {
                "Effect": "Allow",
                "Action": ["s3:GetObject"],
                "Resource": [format!("{bucket}/*/metadata/*")],
            },
            {
                "Effect": "Allow",
                "Action": [
                    "admin:GetTableCatalog", "admin:GetTableBucket", "admin:GetTableNamespace",
                    "admin:GetTable", "admin:GetTableMetadata", "admin:GetTableMetadataLocation",
                ],
                "Resource": [&bucket, format!("{bucket}/*")],
            },
        ],
    })
    .to_string()
}

/// An extraction's policy (decisions 0037 and 0061): read the warehouse;
/// create and commit tables in `tables`' namespaces only (bronze's and
/// silver's), writing on the catalog's record of each and on no data file;
/// and, once a table exists, write its files (its prefix, `tables/<uuid>`,
/// as the catalog says). RustFS refuses a statement that mixes action
/// families, so each family has its own.
pub fn extract_policy(warehouse: &str, tables: &[(&str, Option<&str>)]) -> String {
    let resources = [
        format!("arn:aws:s3:::{warehouse}"),
        format!("arn:aws:s3:::{warehouse}/*"),
    ];
    let mut writes: Vec<String> = tables
        .iter()
        .map(|(namespace, _)| format!("arn:aws:s3:::{warehouse}/.*/namespaces/{namespace}/*"))
        .collect();
    for prefix in tables.iter().filter_map(|(_, prefix)| *prefix) {
        writes.push(format!("arn:aws:s3:::{warehouse}/{prefix}/*"));
    }
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Effect": "Allow",
                "Action": [
                    "s3:GetBucketLocation", "s3:ListBucket", "s3:GetObject",
                    "s3:ListBucketMultipartUploads",
                ],
                "Resource": resources,
            },
            {
                "Effect": "Allow",
                "Action": [
                    "s3:PutObject", "s3:DeleteObject", "s3:AbortMultipartUpload",
                    "s3:ListMultipartUploadParts",
                ],
                "Resource": writes,
            },
            {
                "Effect": "Allow",
                "Action": [
                    "admin:GetTableCatalog", "admin:GetTableBucket", "admin:GetTableNamespace",
                    "admin:GetTable", "admin:GetTableMetadata", "admin:GetTableMetadataLocation",
                    "admin:CreateTable", "admin:CommitTable", "admin:SetTableMetadata",
                    "admin:SetTableMetadataLocation",
                ],
                "Resource": resources,
            },
        ],
    })
    .to_string()
}

/// A maintain job's policy (decisions 0039 and 0061): read the warehouse;
/// commit to and write or delete the files of `tables` (namespace, name,
/// prefix `tables/<uuid>`) only, as the catalog says where they are: its
/// dataset's bronze and silver tables; create nothing.
pub fn maintain_policy(warehouse: &str, tables: &[(String, String, String)]) -> String {
    let resources = [
        format!("arn:aws:s3:::{warehouse}"),
        format!("arn:aws:s3:::{warehouse}/*"),
    ];
    let mut writes = Vec::with_capacity(tables.len() * 2);
    for (namespace, table, prefix) in tables {
        writes.push(format!("arn:aws:s3:::{warehouse}/{prefix}/*"));
        writes.push(format!(
            "arn:aws:s3:::{warehouse}/.*/namespaces/{namespace}/tables/{table}/*"
        ));
    }
    let mut statements = vec![
        json!({
            "Effect": "Allow",
            "Action": ["s3:GetBucketLocation", "s3:ListBucket", "s3:GetObject"],
            "Resource": resources,
        }),
        json!({
            "Effect": "Allow",
            "Action": [
                "admin:GetTableCatalog", "admin:GetTableBucket", "admin:GetTableNamespace",
                "admin:GetTable", "admin:GetTableMetadata", "admin:GetTableMetadataLocation",
                "admin:CommitTable", "admin:SetTableMetadata", "admin:SetTableMetadataLocation",
            ],
            "Resource": resources,
        }),
    ];
    // A dataset with no table yet: nothing to write.
    if !writes.is_empty() {
        statements.push(json!({
            "Effect": "Allow",
            "Action": [
                "s3:PutObject", "s3:DeleteObject", "s3:AbortMultipartUpload",
                "s3:ListMultipartUploadParts",
            ],
            "Resource": writes,
        }));
    }
    json!({ "Version": "2012-10-17", "Statement": statements }).to_string()
}

/// Creating namespaces in the warehouse's catalog, nothing else: the
/// application's part before an extraction (decision 0037).
pub fn namespace_policy(warehouse: &str) -> String {
    let resources = [
        format!("arn:aws:s3:::{warehouse}"),
        format!("arn:aws:s3:::{warehouse}/*"),
    ];
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": [
                "admin:GetTableCatalog", "admin:GetTableBucket", "admin:GetTableNamespace",
                "admin:SetTableNamespace",
            ],
            "Resource": resources,
        }],
    })
    .to_string()
}

/// Deleting one table of the warehouse (decision 0055): its files, by its
/// prefix and its ARN, with the table's write actions RustFS wants for
/// them; then dropping it and its namespace, actions RustFS matches only
/// against the whole warehouse (`tools/verify/lake_drop.py`).
pub fn drop_policy(warehouse: &str, namespace: &str, table: &str, prefix: &str) -> String {
    let bucket = format!("arn:aws:s3:::{warehouse}");
    let resources = [bucket.clone(), format!("{bucket}/*")];
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Effect": "Allow",
                "Action": ["s3:GetBucketLocation", "s3:ListBucket"],
                "Resource": [bucket],
            },
            {
                "Effect": "Allow",
                "Action": ["s3:GetObject", "s3:DeleteObject"],
                "Resource": [
                    format!("arn:aws:s3:::{warehouse}/{prefix}/*"),
                    format!("arn:aws:s3:::{warehouse}/.*/namespaces/{namespace}/tables/{table}/*"),
                ],
            },
            {
                "Effect": "Allow",
                "Action": [
                    "admin:GetTableCatalog", "admin:GetTableBucket", "admin:GetTableNamespace",
                    "admin:GetTable", "admin:GetTableMetadata", "admin:GetTableMetadataLocation",
                    "admin:CommitTable", "admin:SetTableMetadata", "admin:SetTableMetadataLocation",
                    "admin:DeleteTable", "admin:DeleteTableNamespace",
                ],
                "Resource": resources,
            },
        ],
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_only_policy_gets_and_lists_only() {
        let w = "qd-tenant-00000000000000000000000000000001";
        let policy: serde_json::Value = serde_json::from_str(&read_only_policy(w)).unwrap();
        let actions: Vec<&str> = policy["Statement"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|s| s["Action"].as_array().unwrap())
            .map(|a| a.as_str().unwrap())
            .collect();
        assert!(
            actions
                .iter()
                .all(|a| a.contains("Get") || a.contains("List")),
            "{actions:?}"
        );
        assert_eq!(
            policy["Statement"][0]["Resource"][1],
            format!("arn:aws:s3:::{w}/*")
        );
    }

    #[test]
    fn a_listing_reads_the_tables_metadata_and_no_data_file() {
        let policy: serde_json::Value =
            serde_json::from_str(&metadata_policy("qd-tenant-x")).unwrap();
        let objects: Vec<&str> = policy["Statement"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| s["Action"][0] == "s3:GetObject")
            .flat_map(|s| s["Resource"].as_array().unwrap())
            .map(|r| r.as_str().unwrap())
            .collect();
        assert_eq!(objects, ["arn:aws:s3:::qd-tenant-x/*/metadata/*"]);
    }

    #[test]
    fn a_maintain_job_writes_its_tables_files_and_creates_nothing() {
        let table = |ns: &str, prefix: &str| (ns.to_string(), "t1".to_string(), prefix.to_string());
        let tables = [
            table("loans_bronze", "tables/u-1"),
            table("loans_silver", "tables/u-2"),
        ];
        let policy: serde_json::Value =
            serde_json::from_str(&maintain_policy("qd-w", &tables)).unwrap();
        assert_eq!(
            policy["Statement"][2]["Resource"],
            json!([
                "arn:aws:s3:::qd-w/tables/u-1/*",
                "arn:aws:s3:::qd-w/.*/namespaces/loans_bronze/tables/t1/*",
                "arn:aws:s3:::qd-w/tables/u-2/*",
                "arn:aws:s3:::qd-w/.*/namespaces/loans_silver/tables/t1/*"
            ])
        );
        assert!(!policy.to_string().contains("CreateTable"));
        let empty: serde_json::Value = serde_json::from_str(&maintain_policy("qd-w", &[])).unwrap();
        assert_eq!(
            empty["Statement"].as_array().unwrap().len(),
            2,
            "nothing to write"
        );
    }

    #[test]
    fn an_extraction_writes_its_namespaces_record_and_only_its_tables_files() {
        let writes = |prefix| {
            let policy: serde_json::Value =
                serde_json::from_str(&extract_policy("qd-w", &[("loans_bronze", prefix)])).unwrap();
            policy["Statement"][1]["Resource"].clone()
        };
        assert_eq!(
            writes(None),
            json!(["arn:aws:s3:::qd-w/.*/namespaces/loans_bronze/*"])
        );
        assert_eq!(
            writes(Some("tables/u-1")),
            json!([
                "arn:aws:s3:::qd-w/.*/namespaces/loans_bronze/*",
                "arn:aws:s3:::qd-w/tables/u-1/*"
            ])
        );
    }

    #[test]
    fn an_extraction_writes_bronze_and_silver_and_no_other_namespace() {
        let tables = [("loans_bronze", Some("tables/u-1")), ("loans_silver", None)];
        let policy: serde_json::Value =
            serde_json::from_str(&extract_policy("qd-w", &tables)).unwrap();
        assert_eq!(
            policy["Statement"][1]["Resource"],
            json!([
                "arn:aws:s3:::qd-w/.*/namespaces/loans_bronze/*",
                "arn:aws:s3:::qd-w/.*/namespaces/loans_silver/*",
                "arn:aws:s3:::qd-w/tables/u-1/*"
            ])
        );
    }
}
