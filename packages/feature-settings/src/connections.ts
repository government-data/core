/**
 * The database connections of Global variables: the catalog of engines with
 * the fields of each. The server only checks the shape (see
 * `crates/platform/src/connection.rs`); here is each engine's form.
 *
 * The secret fields (password, token, key) are stored only encrypted and
 * never go back to the browser: when editing they stay empty and, if they are
 * not written, they are kept.
 */

export type ConnectionGroup = "sql" | "cloud" | "nosql" | "file" | "generic";

export interface ConnectionField {
  key: string;
  kind: "text" | "port" | "secret" | "textarea";
  required?: boolean;
  placeholder?: string;
}

export interface Engine {
  id: string;
  label: string;
  group: ConnectionGroup;
  /** The default port, if it has one. */
  port?: number;
  fields: ConnectionField[];
}

/** The secret fields (like `SECRET_FIELDS` in the server). */
export const SECRET_FIELDS = ["password", "token", "secret_key", "key_json", "connection_string"];

const host = (placeholder = "servidor"): ConnectionField => ({
  key: "host",
  kind: "text",
  required: true,
  placeholder,
});
const port: ConnectionField = { key: "port", kind: "port" };
const database: ConnectionField = { key: "database", kind: "text" };
const user: ConnectionField = { key: "user", kind: "text" };
const password: ConnectionField = { key: "password", kind: "secret" };
const options: ConnectionField = { key: "options", kind: "text", placeholder: "encrypt=true" };

/** A server SQL engine: host, port, database, user, password and JDBC options. */
const sql = (id: string, label: string, portNumber: number, placeholder?: string): Engine => ({
  id,
  label,
  group: "sql",
  port: portNumber,
  fields: [host(placeholder), port, database, user, password, options],
});

export const ENGINES: Engine[] = [
  sql("sqlserver", "Microsoft SQL Server", 1433),
  sql("azure_sql", "Azure SQL Database", 1433, "servidor.database.windows.net"),
  sql("azure_sql_mi", "Azure SQL Managed Instance", 1433, "instancia.xxxx.database.windows.net"),
  sql("synapse", "Azure Synapse Analytics", 1433, "espacio.sql.azuresynapse.net"),
  sql("postgres", "PostgreSQL", 5432),
  sql(
    "azure_postgres",
    "Azure Database for PostgreSQL",
    5432,
    "servidor.postgres.database.azure.com",
  ),
  sql("mysql", "MySQL", 3306),
  sql("mariadb", "MariaDB", 3306),
  sql("azure_mysql", "Azure Database for MySQL", 3306, "servidor.mysql.database.azure.com"),
  {
    id: "oracle",
    label: "Oracle",
    group: "sql",
    port: 1521,
    fields: [
      host(),
      port,
      { key: "service", kind: "text", placeholder: "ORCLPDB1" },
      { key: "sid", kind: "text" },
      user,
      password,
      options,
    ],
  },
  sql("db2", "IBM Db2", 50000),
  sql("hana", "SAP HANA", 30015),
  sql("teradata", "Teradata", 1025),
  sql("vertica", "Vertica", 5433),
  sql("exasol", "Exasol", 8563),
  sql("redshift", "Amazon Redshift", 5439, "cluster.xxxx.us-east-1.redshift.amazonaws.com"),
  {
    id: "snowflake",
    label: "Snowflake",
    group: "cloud",
    fields: [
      { key: "account", kind: "text", required: true, placeholder: "xy12345.us-east-1" },
      { key: "warehouse", kind: "text" },
      database,
      { key: "schema", kind: "text" },
      { key: "role", kind: "text" },
      user,
      password,
      options,
    ],
  },
  {
    id: "databricks",
    label: "Databricks",
    group: "cloud",
    fields: [
      host("adb-xxxx.azuredatabricks.net"),
      { key: "http_path", kind: "text", required: true, placeholder: "/sql/1.0/warehouses/…" },
      { key: "catalog", kind: "text" },
      { key: "token", kind: "secret", required: true },
    ],
  },
  {
    id: "bigquery",
    label: "Google BigQuery",
    group: "cloud",
    fields: [
      { key: "project", kind: "text", required: true },
      { key: "dataset", kind: "text" },
      { key: "key_json", kind: "textarea", required: true },
    ],
  },
  {
    id: "athena",
    label: "Amazon Athena",
    group: "cloud",
    fields: [
      { key: "region", kind: "text", required: true, placeholder: "us-east-1" },
      { key: "s3_staging", kind: "text", required: true, placeholder: "s3://bucket/athena/" },
      { key: "workgroup", kind: "text" },
      { key: "access_key", kind: "text" },
      { key: "secret_key", kind: "secret" },
    ],
  },
  {
    id: "mongodb",
    label: "MongoDB",
    group: "nosql",
    port: 27017,
    fields: [host(), port, database, user, password, options],
  },
  {
    id: "cosmos_mongo",
    label: "Azure Cosmos DB (API de MongoDB)",
    group: "nosql",
    fields: [
      {
        key: "connection_string",
        kind: "secret",
        required: true,
        placeholder: "mongodb://…cosmos.azure.com:10255/?ssl=true",
      },
      database,
    ],
  },
  {
    id: "neo4j",
    label: "Neo4j",
    group: "nosql",
    fields: [
      { key: "url", kind: "text", required: true, placeholder: "bolt://servidor:7687" },
      database,
      user,
      password,
    ],
  },
  {
    id: "sqlite",
    label: "SQLite",
    group: "file",
    fields: [{ key: "path", kind: "text", required: true, placeholder: "D:\\datos\\base.sqlite" }],
  },
  {
    id: "h2",
    label: "H2",
    group: "file",
    fields: [{ key: "path", kind: "text", required: true }, user, password],
  },
  {
    id: "access",
    label: "Microsoft Access",
    group: "file",
    fields: [{ key: "path", kind: "text", required: true, placeholder: "D:\\datos\\base.accdb" }],
  },
  {
    id: "jdbc",
    label: "Otra (JDBC)",
    group: "generic",
    fields: [
      { key: "url", kind: "text", required: true, placeholder: "jdbc:…" },
      { key: "driver", kind: "text", placeholder: "com.empresa.Driver" },
      user,
      password,
    ],
  },
];

export const GROUPS: ConnectionGroup[] = ["sql", "cloud", "nosql", "file", "generic"];

export const engineById = (id: string | undefined) => ENGINES.find((e) => e.id === id);

/** The value sent to `/secrets`: only the fields with text. */
export function connectionValue(engine: string, fields: Record<string, string>) {
  const clean: Record<string, string> = {};
  for (const [k, v] of Object.entries(fields)) if (v.trim()) clean[k] = v.trim();
  return { kind: "database", engine, fields: clean };
}

/** What is missing to save: the empty required fields (the saved secrets count). */
export function missingFields(
  engine: Engine,
  fields: Record<string, string>,
  saved: string[] = [],
) {
  return engine.fields
    .filter((f) => f.required && !fields[f.key]?.trim() && !saved.includes(f.key))
    .map((f) => f.key);
}
