//! Database connections: a global secret of the connection kind
//! (`{"kind": "database", "engine": "sqlserver", "fields": {...}}`) registered
//! once in Global variables with its engine's form.
//!
//! What is not secret (engine, server, port, database…) is also stored in
//! clear to show and edit it; user and password, or the token, only
//! encrypted. A flow that picks it as variable `X` is meant to get `X_host`,
//! `X_port`, `X_database`… and `X_url` with the JDBC URL (for the engines
//! that have one): see [`Connection::variables`].

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

/// The `kind` of a connection secret.
pub const CONNECTION_KIND: &str = "database";
/// The secret fields: only stored encrypted and never sent back to the browser.
const SECRET_FIELDS: &[&str] = &[
    "password",
    "token",
    "secret_key",
    "key_json",
    "connection_string",
];
/// The cap on fields, and on each one's characters.
const MAX_FIELDS: usize = 30;
const MAX_FIELD_CHARS: usize = 8_000;

/// A connection: its engine and its fields, as the administrator wrote them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    pub engine: String,
    pub fields: BTreeMap<String, String>,
}

/// Whether it is secret (not shown, and not passed as a flow variable in clear).
fn is_secret_field(key: &str) -> bool {
    SECRET_FIELDS.contains(&key)
}

fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 40
        && key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

impl Connection {
    /// The connection of a secret's value, if it is one.
    pub fn from_value(value: &Value) -> Option<Self> {
        let map = value.as_object()?;
        if map.get("kind")?.as_str()? != CONNECTION_KIND {
            return None;
        }
        let engine = map.get("engine")?.as_str()?.to_string();
        let fields = map
            .get("fields")?
            .as_object()?
            .iter()
            .map(|(k, v)| {
                let text = match v {
                    Value::String(s) => s.clone(),
                    Value::Null => String::new(),
                    other => other.to_string(),
                };
                (k.clone(), text)
            })
            .collect();
        Some(Self { engine, fields })
    }

    /// The value that is encrypted and handed to the engine.
    pub fn to_value(&self) -> Value {
        json!({ "kind": CONNECTION_KIND, "engine": self.engine, "fields": self.fields })
    }

    /// That the engine and the fields have an acceptable shape. Each engine's
    /// form is in the web; here only the shape and the caps are checked.
    pub fn validate(&self) -> Result<(), String> {
        if !valid_key(&self.engine) {
            return Err(format!("motor inválido: {}", self.engine));
        }
        if self.fields.len() > MAX_FIELDS {
            return Err("demasiados campos".into());
        }
        for (k, v) in &self.fields {
            if !valid_key(k) {
                return Err(format!("campo inválido: {k}"));
            }
            if v.chars().count() > MAX_FIELD_CHARS {
                return Err(format!("el campo {k} es demasiado largo"));
            }
        }
        if let Some(port) = self.fields.get("port").filter(|p| !p.trim().is_empty()) {
            if port.trim().parse::<u16>().is_err() {
                return Err(format!("puerto inválido: {port}"));
            }
        }
        Ok(())
    }

    /// What is stored in clear: the engine, the fields that are not secret,
    /// and which secrets it has (without their values).
    pub fn public(&self) -> Value {
        let mut fields = Map::new();
        let mut secrets = Vec::new();
        for (k, v) in &self.fields {
            if is_secret_field(k) {
                if !v.is_empty() {
                    secrets.push(Value::from(k.as_str()));
                }
            } else {
                fields.insert(k.clone(), Value::from(v.as_str()));
            }
        }
        json!({ "engine": self.engine, "fields": fields, "secrets": secrets })
    }

    /// When editing: a secret field that arrives empty keeps the previous one
    /// (the browser never has it to send back).
    pub fn keep_secrets_from(mut self, before: &Connection) -> Self {
        for key in SECRET_FIELDS {
            if self.fields.get(*key).is_none_or(|v| v.is_empty()) {
                if let Some(old) = before.fields.get(*key).filter(|v| !v.is_empty()) {
                    self.fields.insert((*key).to_string(), old.clone());
                }
            }
        }
        self
    }

    fn field(&self, key: &str) -> Option<&str> {
        self.fields
            .get(key)
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
    }

    /// The JDBC URL, for the server SQL engines that have one.
    pub fn jdbc_url(&self) -> Option<String> {
        let host = self.field("host")?;
        let port = self.field("port");
        let db = self.field("database").unwrap_or("");
        let hp = |default: &str| format!("{host}:{}", port.unwrap_or(default));
        let opts = self.field("options").unwrap_or("");
        let url = match self.engine.as_str() {
            "sqlserver" | "azure_sql" | "azure_sql_mi" | "synapse" => {
                let mut u = format!("jdbc:sqlserver://{}", hp("1433"));
                if !db.is_empty() {
                    u.push_str(&format!(";databaseName={db}"));
                }
                if self.engine.starts_with("azure") || self.engine == "synapse" {
                    u.push_str(";encrypt=true;trustServerCertificate=false");
                }
                if !opts.is_empty() {
                    u.push(';');
                    u.push_str(opts.trim_start_matches(';'));
                }
                return Some(u);
            }
            "postgres" | "azure_postgres" => format!("jdbc:postgresql://{}/{db}", hp("5432")),
            "redshift" => format!("jdbc:redshift://{}/{db}", hp("5439")),
            "mysql" | "azure_mysql" => format!("jdbc:mysql://{}/{db}", hp("3306")),
            "mariadb" => format!("jdbc:mariadb://{}/{db}", hp("3306")),
            "oracle" => match (self.field("service"), self.field("sid")) {
                (Some(service), _) => format!("jdbc:oracle:thin:@//{}/{service}", hp("1521")),
                (None, Some(sid)) => format!("jdbc:oracle:thin:@{}:{sid}", hp("1521")),
                (None, None) => format!("jdbc:oracle:thin:@//{}/{db}", hp("1521")),
            },
            "db2" => format!("jdbc:db2://{}/{db}", hp("50000")),
            "hana" => format!("jdbc:sap://{}/?databaseName={db}", hp("30015")),
            "teradata" => format!(
                "jdbc:teradata://{host}/DATABASE={db},DBS_PORT={}",
                port.unwrap_or("1025")
            ),
            "vertica" => format!("jdbc:vertica://{}/{db}", hp("5433")),
            "exasol" => format!("jdbc:exa:{}", hp("8563")),
            _ => return None,
        };
        Some(if opts.is_empty() {
            url
        } else if url.contains('?') {
            format!("{url}&{}", opts.trim_start_matches(['?', '&']))
        } else {
            format!("{url}?{}", opts.trim_start_matches(['?', '&']))
        })
    }

    /// The flow variables of connection `name`: `name_engine`, one
    /// `name_<field>` per field that is not secret (the port as an integer)
    /// and `name_url` if there is a JDBC URL. The connection string (MongoDB,
    /// Cosmos DB) goes as a variable because its connectors ask for it that
    /// way; user and password are not included.
    pub fn variables(&self, name: &str) -> Vec<(String, Value)> {
        let mut out = vec![(format!("{name}_engine"), Value::from(self.engine.as_str()))];
        for (k, v) in &self.fields {
            if v.is_empty() || (is_secret_field(k) && k != "connection_string") {
                continue;
            }
            let value = match (k.as_str(), v.trim().parse::<i64>()) {
                ("port", Ok(n)) => Value::from(n),
                _ => Value::from(v.as_str()),
            };
            out.push((format!("{name}_{k}"), value));
        }
        if let Some(url) = self.jdbc_url() {
            out.push((format!("{name}_url"), Value::from(url)));
        }
        out
    }

    /// The environment of connection `name` for a process that connects by
    /// itself (a Python flow): what [`Connection::variables`] gives, as text,
    /// plus the secret fields (`name_password`, `name_token`…). Each one says
    /// whether it is secret, so the log masks it.
    pub fn environment(&self, name: &str) -> Vec<(String, String, bool)> {
        let mut out: Vec<(String, String, bool)> = self
            .variables(name)
            .into_iter()
            .map(|(key, value)| {
                let secret = key == format!("{name}_connection_string");
                let text = value
                    .as_str()
                    .map_or_else(|| value.to_string(), str::to_string);
                (key, text, secret)
            })
            .collect();
        for (k, v) in &self.fields {
            if !v.is_empty() && is_secret_field(k) && k != "connection_string" {
                out.push((format!("{name}_{k}"), v.clone(), true));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(engine: &str, fields: &[(&str, &str)]) -> Connection {
        Connection {
            engine: engine.into(),
            fields: fields
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn the_environment_carries_the_secret_fields_marked() {
        let c = conn(
            "postgres",
            &[
                ("host", "db1"),
                ("port", "5432"),
                ("database", "cartera"),
                ("user", "lector"),
                ("password", "s3cr3t"),
            ],
        );
        let env = c.environment("CORE");
        let get = |k: &str| env.iter().find(|(key, ..)| key == k).cloned();
        assert_eq!(
            get("CORE_host"),
            Some(("CORE_host".into(), "db1".into(), false))
        );
        assert_eq!(
            get("CORE_port"),
            Some(("CORE_port".into(), "5432".into(), false))
        );
        assert_eq!(
            get("CORE_user"),
            Some(("CORE_user".into(), "lector".into(), false))
        );
        assert_eq!(
            get("CORE_password"),
            Some(("CORE_password".into(), "s3cr3t".into(), true))
        );
        assert_eq!(get("CORE_engine").map(|e| e.1), Some("postgres".into()));
        assert!(
            get("CORE_url").is_some_and(|u| u.1.starts_with("jdbc:postgresql://db1:5432/cartera"))
        );
    }

    #[test]
    fn reads_only_connection_values() {
        let v = json!({"kind": "database", "engine": "sqlserver", "fields": {"host": "Hosthidra", "port": 1433}});
        let c = Connection::from_value(&v).unwrap();
        assert_eq!(c.fields["port"], "1433");
        assert_eq!(Connection::from_value(&c.to_value()), Some(c));
        assert!(Connection::from_value(&json!("text")).is_none());
        assert!(Connection::from_value(&json!({"kind": "other"})).is_none());
    }

    #[test]
    fn validates_shape_and_port() {
        assert!(conn("sqlserver", &[("host", "h"), ("port", "1433")])
            .validate()
            .is_ok());
        assert!(conn("Sql Server", &[]).validate().is_err());
        assert!(conn("sqlserver", &[("Host", "h")]).validate().is_err());
        assert!(conn("sqlserver", &[("port", "one")]).validate().is_err());
        assert!(conn("sqlserver", &[("port", "70000")]).validate().is_err());
    }

    #[test]
    fn public_part_has_no_secret_values() {
        let c = conn(
            "sqlserver",
            &[
                ("host", "h"),
                ("user", "diego"),
                ("password", "x"),
                ("token", ""),
            ],
        );
        let p = c.public();
        assert_eq!(p["fields"], json!({"host": "h", "user": "diego"}));
        assert_eq!(p["secrets"], json!(["password"]));
        assert!(!p.to_string().contains("\"x\""));
    }

    #[test]
    fn editing_keeps_secrets_left_empty() {
        let before = conn("sqlserver", &[("host", "a"), ("password", "old")]);
        let after =
            conn("sqlserver", &[("host", "b"), ("password", "")]).keep_secrets_from(&before);
        assert_eq!(after.fields["password"], "old");
        assert_eq!(after.fields["host"], "b");
        let changed = conn("sqlserver", &[("password", "new")]).keep_secrets_from(&before);
        assert_eq!(changed.fields["password"], "new");
    }

    #[test]
    fn expands_into_variables_and_url() {
        let c = conn(
            "sqlserver",
            &[
                ("host", "Hosthidra"),
                ("port", "1433"),
                ("database", "DMInformes"),
                ("user", "diego"),
                ("password", "x"),
            ],
        );
        let vars: BTreeMap<String, Value> = c.variables("BD").into_iter().collect();
        assert_eq!(vars["BD_host"], json!("Hosthidra"));
        assert_eq!(vars["BD_port"], json!(1433));
        assert_eq!(vars["BD_user"], json!("diego"));
        assert_eq!(
            vars["BD_url"],
            json!("jdbc:sqlserver://Hosthidra:1433;databaseName=DMInformes")
        );
        assert!(!vars.contains_key("BD_password"));
    }

    #[test]
    fn builds_urls_for_the_main_engines() {
        let url = |e: &str, f: &[(&str, &str)]| conn(e, f).jdbc_url();
        assert_eq!(
            url("postgres", &[("host", "h"), ("database", "d")]).unwrap(),
            "jdbc:postgresql://h:5432/d"
        );
        assert_eq!(
            url("oracle", &[("host", "h"), ("service", "ORCL")]).unwrap(),
            "jdbc:oracle:thin:@//h:1521/ORCL"
        );
        assert_eq!(
            url("azure_sql", &[("host", "x.database.windows.net"), ("database", "d")]).unwrap(),
            "jdbc:sqlserver://x.database.windows.net:1433;databaseName=d;encrypt=true;trustServerCertificate=false"
        );
        assert_eq!(
            url(
                "mysql",
                &[("host", "h"), ("database", "d"), ("options", "useSSL=true")]
            )
            .unwrap(),
            "jdbc:mysql://h:3306/d?useSSL=true"
        );
        assert_eq!(url("mongodb", &[("host", "h")]), None);
    }

    #[test]
    fn mongo_gets_its_connection_string() {
        let mongo = conn("cosmos_mongo", &[("connection_string", "mongodb://u:p@h")]);
        let vars: BTreeMap<String, Value> = mongo.variables("M").into_iter().collect();
        assert_eq!(vars["M_connection_string"], json!("mongodb://u:p@h"));
    }
}
