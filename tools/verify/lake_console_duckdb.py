# /// script
# requires-python = ">=3.11"
# dependencies = ["duckdb>=1.4", "boto3"]
# ///
"""Verification for decision 0054: a console statement runs on a worker's
DuckDB with the lake attached through read-only STS credentials and locked
down as `Runner::attach_lake` locks it (only the warehouse's paths, no
other external access, configuration locked), wrapped as
`SELECT * FROM (<statement>) AS q LIMIT 201`. It reads the lake; it cannot
read the worker's files or the web, attach, install, set, write, or see the
secret. Counts and metadata only; no value is read, credentials are never
printed.

From the repository root, with the dev stack up and a cut loaded:

    uv run tools/verify/lake_console_duckdb.py
"""
import json, time
from pathlib import Path

import boto3, duckdb

ROOT = Path(__file__).resolve().parents[2]
env = dict(l.split("=", 1) for l in (ROOT / ".env").read_text().splitlines() if "=" in l and not l.startswith("#"))
ISSUER = (env["RUSTFS_STS_ISSUER_ACCESS_KEY"], env["RUSTFS_STS_ISSUER_SECRET_KEY"])
CA = str(ROOT / "infra/traefik/certs/dev-ca.crt")
BASE = "https://localhost"
WAREHOUSE = "qd-tenant-00000000000000000000000000000001"
TABLE = "clientes_naturales_bronze.clientes_naturales"
results = []


def check(name, expect_ok, sql):
    """Runs `sql` wrapped as the console wraps it; OK when it is taken or refused as expected."""
    t = time.time()
    wrapped = f"SELECT * FROM ({sql}) AS q LIMIT 201"
    try:
        rows = C["con"].execute(wrapped).fetchall()
        outcome = f"taken, {len(rows)} rows"
        ok = expect_ok
    except Exception as e:
        outcome = f"refused ({type(e).__name__}: {str(e).splitlines()[0][:110]})"
        ok = not expect_ok
    results.append(("OK" if ok else "FAIL", name, outcome, time.time() - t))


def read_only(warehouse):
    arns = [f"arn:aws:s3:::{warehouse}", f"arn:aws:s3:::{warehouse}/*"]
    return {"Version": "2012-10-17", "Statement": [
        {"Effect": "Allow", "Action": ["s3:GetBucketLocation", "s3:ListBucket", "s3:GetObject"], "Resource": arns},
        {"Effect": "Allow", "Action": ["admin:GetTableCatalog", "admin:GetTableBucket", "admin:GetTableNamespace",
                                       "admin:GetTable", "admin:GetTableMetadata", "admin:GetTableMetadataLocation"],
         "Resource": arns}]}


sts = boto3.client("sts", endpoint_url=BASE, aws_access_key_id=ISSUER[0], aws_secret_access_key=ISSUER[1],
                   region_name="us-east-1", verify=CA)
c = sts.assume_role(RoleArn="arn:aws:iam::000000000000:role/qd-job", RoleSessionName="console-verify",
                    DurationSeconds=900, Policy=json.dumps(read_only(WAREHOUSE)))["Credentials"]
ak, sk, tok = c["AccessKeyId"], c["SecretAccessKey"], c["SessionToken"]
con = duckdb.connect()
# As Runner::attach_lake does it.
con.execute("INSTALL avro; LOAD avro; INSTALL iceberg; LOAD iceberg; INSTALL httpfs; LOAD httpfs;")
con.execute(f"SET ca_cert_file='{CA}'")
con.execute(f"""CREATE SECRET lake_job (TYPE s3, KEY_ID '{ak}', SECRET '{sk}', SESSION_TOKEN '{tok}',
                REGION 'us-east-1', ENDPOINT 'localhost', URL_STYLE 'path', USE_SSL true)""")
con.execute(f"""ATTACH '{WAREHOUSE}' AS lake (TYPE iceberg, URI '{BASE}/iceberg',
                AUTHORIZATION_TYPE 'sigv4', SIGV4_SERVICE 's3', SIGV4_REGION 'us-east-1', SECRET lake_job)""")
con.execute(f"""SET allowed_directories = ['{BASE}/iceberg/v1/{WAREHOUSE}/', 's3://{WAREHOUSE}/'];
                SET enable_external_access = false; SET lock_configuration = true;""")
C = {"con": con}
print("duckdb", duckdb.__version__)

check("a count of a bronze table", True, f"SELECT count(*) AS n FROM lake.{TABLE}")
check("an aggregate by group (counts only)", True,
      f"SELECT count(*) AS n FROM lake.{TABLE} GROUP BY cut")
check("the lake's tables (metadata)", True,
      "SELECT schema_name, table_name FROM duckdb_tables() WHERE database_name = 'lake'")
check("a join of two tables of the lake", True,
      f"SELECT count(*) AS n FROM lake.{TABLE} a JOIN lake.{TABLE} b ON a.cut = b.cut AND false")
check("a local file of the worker", False, "SELECT * FROM read_text('/etc/hostname')")
check("a file on the web", False, "SELECT * FROM read_csv('https://example.org/x.csv')")
check("another bucket", False, "SELECT * FROM read_parquet('s3://qd-tenant-other/x.parquet')")
check("ATTACH", False, "ATTACH ':memory:' AS other")
check("INSTALL", False, "INSTALL spatial")
check("SET", False, "SET enable_external_access = true")
check("COPY out", False, "COPY (SELECT 1) TO '/tmp/x.csv'")
check("two statements", False, "SELECT 1; DROP TABLE x")
check("a write to the lake", False, f"INSERT INTO lake.{TABLE} SELECT * FROM lake.{TABLE}")
check("a table created", False, "CREATE TABLE t AS SELECT 1")
check("SET through query()", False, "SELECT * FROM query('SET enable_external_access = true')")
check("a local file through query()", False, "SELECT * FROM query('SELECT * FROM read_text(''/etc/hostname'')')")
check("the worker's environment", False, "SELECT getenv('HOME') AS h")
t = time.time()
try:
    s = con.execute("SELECT * FROM (SELECT secret_string FROM duckdb_secrets()) AS q LIMIT 201").fetchall()
    leaked = any(sk in str(r) or tok in str(r) for r in s)
    results.append(("FAIL" if leaked else "OK", "the secret, through duckdb_secrets()",
                    "LEAKED" if leaked else f"redacted ({len(s)} rows)", time.time() - t))
except Exception as e:
    results.append(("OK", "the secret, through duckdb_secrets()", f"refused ({type(e).__name__})", time.time() - t))
t = time.time()
try:
    s = con.execute("SELECT * FROM (SELECT value FROM duckdb_settings() WHERE name LIKE 's3_%') AS q LIMIT 201").fetchall()
    leaked = any(sk in str(r) or tok in str(r) for r in s)
    results.append(("FAIL" if leaked else "OK", "the secret, through duckdb_settings()",
                    "LEAKED" if leaked else "not there", time.time() - t))
except Exception as e:
    results.append(("OK", "the secret, through duckdb_settings()", f"refused ({type(e).__name__})", time.time() - t))

for status, name, detail, secs in results:
    print(f"[{status:4}] {name} ({secs:.1f}s): {detail}")
print("== all checks passed" if all(r[0] == "OK" for r in results) else "== some checks FAILED")
