# /// script
# requires-python = ">=3.11"
# dependencies = ["duckdb>=1.4", "boto3"]
# ///
"""Verification for decision 0043 (point 1, the lake listed as a source): a
job lists its tenant's lake through DuckDB's `iceberg` extension, its
namespaces, tables and columns, from the catalog and the tables' metadata
only. Credentials that may read the tables' metadata files and no data
file still list it all, and cannot count a table's rows: the listing reads
no row. Counts only; no name or value is printed, credentials never.

From the repository root, with the dev stack up and a cut loaded
(tools/dev/lake-load.sh):

    uv run tools/verify/lake_listing.py
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
CATALOG = ["admin:GetTableCatalog", "admin:GetTableBucket", "admin:GetTableNamespace", "admin:GetTable",
           "admin:GetTableMetadata", "admin:GetTableMetadataLocation"]
results = []


def step(name):
    def wrap(fn):
        t = time.time()
        try:
            results.append(("OK", name, fn(), time.time() - t))
        except Exception as e:
            results.append(("FAIL", name, f"{type(e).__name__}: {str(e)[:220]}", time.time() - t))
        return fn
    return wrap


def policy(objects):
    bucket = f"arn:aws:s3:::{WAREHOUSE}"
    arns = [bucket, f"{bucket}/*"]
    return {"Version": "2012-10-17", "Statement": [
        {"Effect": "Allow", "Action": ["s3:GetBucketLocation", "s3:ListBucket"], "Resource": [bucket]},
        {"Effect": "Allow", "Action": ["s3:GetObject"], "Resource": [f"{bucket}/{o}" for o in objects]},
        {"Effect": "Allow", "Action": CATALOG, "Resource": arns}]}


def assume(p):
    sts = boto3.client("sts", endpoint_url=BASE, aws_access_key_id=ISSUER[0], aws_secret_access_key=ISSUER[1],
                       region_name="us-east-1", verify=CA)
    c = sts.assume_role(RoleArn="arn:aws:iam::000000000000:role/qd-job", RoleSessionName="job-verify",
                        DurationSeconds=900, Policy=json.dumps(p))["Credentials"]
    return c["AccessKeyId"], c["SecretAccessKey"], c["SessionToken"]


def duck(creds):
    con = duckdb.connect()
    con.execute("INSTALL iceberg; LOAD iceberg; INSTALL httpfs; LOAD httpfs;")
    con.execute(f"SET ca_cert_file='{CA}'")
    ak, sk, tok = creds
    con.execute(f"""CREATE SECRET s3 (TYPE s3, KEY_ID '{ak}', SECRET '{sk}', SESSION_TOKEN '{tok}',
                    REGION 'us-east-1', ENDPOINT 'localhost', URL_STYLE 'path', USE_SSL true)""")
    con.execute(f"""ATTACH '{WAREHOUSE}' AS lake (TYPE iceberg, URI '{BASE}/iceberg',
                    AUTHORIZATION_TYPE 'sigv4', SIGV4_SERVICE 's3', SIGV4_REGION 'us-east-1', SECRET s3)""")
    return con


def listing(con):
    tables = con.execute("SELECT schema_name, table_name FROM duckdb_tables() WHERE database_name = 'lake'").fetchall()
    columns = con.execute("SELECT schema_name, table_name, count(*) FROM duckdb_columns() "
                          "WHERE database_name = 'lake' GROUP BY ALL").fetchall()
    return tables, columns


C = {}


@step("read-only on the whole warehouse: namespaces, tables and columns (duckdb_tables, duckdb_columns)")
def _():
    C["full"] = listing(duck(assume(policy(["*"]))))
    tables, columns = C["full"]
    return (f"{len({s for s, _ in tables})} namespaces, {len(tables)} tables, "
            f"{sum(n for *_, n in columns)} columns, every table with its columns: "
            f"{len(columns) == len(tables)}")


@step("metadata files only (*/metadata/*): the same listing")
def _():
    con = duck(assume(policy(["*/metadata/*"])))
    C["meta"] = con
    same = listing(con)
    return f"same as with the whole warehouse: {sorted(same[0]) == sorted(C['full'][0]) and sorted(same[1]) == sorted(C['full'][1])}"


@step("columns per table, as listed (a distribution of counts)")
def _():
    per = sorted(n for *_, n in C["full"][1])
    return f"min {per[0]}, max {per[-1]}"


@step("columns of one table after it is read (DESCRIBE)")
def _():
    s, t = C["full"][0][0]
    con = duck(assume(policy(["*"])))
    n = len(con.execute(f'DESCRIBE lake."{s}"."{t}"').fetchall())
    again = con.execute("SELECT count(*) FROM duckdb_columns() WHERE database_name = 'lake' "
                        "AND schema_name = ? AND table_name = ?", [s, t]).fetchone()[0]
    return f"DESCRIBE {n} columns; duckdb_columns after it {again}"


@step("metadata files only: DESCRIBE of every table gives its columns")
def _():
    counts = [len(C["meta"].execute(f'DESCRIBE lake."{s}"."{t}"').fetchall()) for s, t in C["full"][0]]
    return f"{len(counts)} tables, {sum(counts)} columns (min {min(counts)}, max {max(counts)})"


@step("metadata files only: a count needing a data file is denied (no data file read)")
def _():
    s, t = C["full"][0][0]
    try:
        n = C["meta"].execute(f'SELECT count(*) FROM lake."{s}"."{t}" WHERE length(CAST(cut AS VARCHAR)) > 0').fetchone()[0]
        return f"NOT DENIED (unexpected): {'some' if n else 'no'} rows"
    except Exception as e:
        return f"denied ({type(e).__name__}, {'403' if '403' in str(e) else 'AccessDenied' if 'AccessDenied' in str(e) else str(e)[:80]})"


for status, name, detail, secs in results:
    print(f"[{status:4}] {name} ({secs:.1f}s): {detail}")
