//! `server quality catalog import` (decision 0021): a dataset's mapping
//! and its rules files, from YAML in git, into the tenant's catalog on
//! Dolt as one commit on `main`. Importing the same files again makes no
//! commit. The tenant's database must exist (`infra/dolt/tenant.sh add`).
//!
//! ```text
//! server quality catalog import --mapping FILE RULES... [--tenant ID] [--author "Name <email>"]
//! ```

use std::path::Path;

use infrastructure::catalog_dolt::{DatasetImport, DoltCatalog};
use quality::mapping::Mapping;
use texts::{text_in, Language};
use uuid::Uuid;

type Failure = Box<dyn std::error::Error>;

const AUTHOR: &str = "Quality Data <catalog@quality-data.invalid>";

pub(crate) async fn run() -> Result<(), Failure> {
    let lang = Language::Es;
    let usage = || -> Failure { text_in(lang, "quality.cli.catalog.usage", &[]).into() };
    let mut args = std::env::args().skip(3);
    if args.next().as_deref() != Some("import") {
        return Err(usage());
    }
    let (mut mapping, mut files) = (None, Vec::new());
    let mut tenant = tenancy::MAIN_ID.to_string();
    let mut author = AUTHOR.to_string();
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(usage);
        match arg.as_str() {
            "--mapping" => mapping = Some(value()?),
            "--tenant" => tenant = value()?,
            "--author" => author = value()?,
            _ if arg.starts_with("--") => return Err(usage()),
            _ => files.push(arg),
        }
    }
    let path = mapping.ok_or_else(usage)?;
    let tenant = Uuid::parse_str(&tenant).map_err(|_| usage())?;
    let mapping_yaml = std::fs::read_to_string(&path)?;
    let parsed = Mapping::parse(&mapping_yaml)
        .map_err(|e| crate::quality::mapping_error(lang, &path, &e))?;
    let dataset = parsed.dataset.value.clone();
    let rules = rules_files(lang, &dataset, &files)?;
    let catalog = DoltCatalog::from_env()?
        .ok_or_else(|| text_in(lang, "quality.cli.catalog.noCatalog", &[]))?;
    let import = DatasetImport {
        dataset: &dataset,
        description: parsed.description.as_deref(),
        mapping: &mapping_yaml,
        rules: &rules,
    };
    let commit = catalog
        .import(&quality::catalog::database(tenant), &import, &author)
        .await?;
    let count = rules.len().to_string();
    println!(
        "{}",
        text_in(
            lang,
            "quality.cli.catalog.imported",
            &[
                ("dataset", dataset.clone()),
                ("files", count),
                ("commit", commit)
            ]
        )
    );
    Ok(())
}

/// Each file as `(name, yaml)`, read as rules of `dataset`.
fn rules_files(
    lang: Language,
    dataset: &str,
    paths: &[String],
) -> Result<Vec<(String, String)>, Failure> {
    let mut rules = Vec::new();
    for path in paths {
        let yaml = std::fs::read_to_string(path)?;
        let file = quality_rules::parse(&yaml).map_err(|e| format!("{path}: {}", e.message))?;
        if file.dataset.value != dataset {
            let other = file.dataset.value.clone();
            return Err(text_in(
                lang,
                "quality.cli.catalog.otherDataset",
                &[
                    ("file", path.clone()),
                    ("dataset", other),
                    ("mapping", dataset.to_string()),
                ],
            )
            .into());
        }
        let name = Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(path)
            .to_string();
        rules.push((name, yaml));
    }
    Ok(rules)
}
