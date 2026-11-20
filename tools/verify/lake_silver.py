# /// script
# requires-python = ">=3.11"
# dependencies = ["duckdb==2.0.0", "boto3"]
# ///
"""Verification for decision 0061: a materialized silver layer.

1. DuckDB (the worker's version) creates an Iceberg table, partitioned by
   `cut`, in a second namespace of the dataset (`<dataset>_silver`) with
   credentials from STS as the platform issues them, and writes a cut there
   typed and normalized from the bronze table's cut.
2. Replacing a silver cut (delete its rows, insert them again, one
   transaction) twice leaves the same rows: a retried job.
3. Whether a column can be added to the silver table (a mapping's new
   field) with `ALTER TABLE`.
4. Whether the catalog's actions (`admin:CreateTable`, `admin:CommitTable`)
   can be scoped to one namespace, rather than the whole warehouse; and
   that credentials for the silver namespace cannot write bronze.

Only namespaces named `verify_silver_*` are made; the rows are generated.
Counts, names and outcomes are printed, never a value, never a credential.
From the repository root, with the dev stack up:

    uv run tools/verify/lake_silver.py

(`QD_ROOT` points at the checkout holding `.env`, from a worktree.)
"""
import json, os, time
from pathlib import Path

import boto3, duckdb

ROOT = Path(os.environ.get("QD_ROOT", Path(__file__).resolve().parents[2]))
env = dict(l.split("=", 1) for l in (ROOT / ".env").read_text().splitlines() if "=" in l and not l.startswith("#"))
ISSUER = (env["RUSTFS_STS_ISSUER_ACCESS_KEY"], env["RUSTFS_STS_ISSUER_SECRET_KEY"])
LOADER = (env["LAKE_LOADER_ACCESS_KEY"], env["LAKE_LOADER_SECRET_KEY"], None)
CA = str(ROOT / "infra/traefik/certs/dev-ca.crt")
BASE = "https://localhost"
W = "qd-tenant-00000000000000000000000000000001"
BRONZE, SILVER, OTHER = "verify_silver_bronze", "verify_silver_silver", "verify_silver_other_silver"
CUT = "2026-10-31"
results, C = [], {}
READ = ["s3:GetBucketLocation", "s3:ListBucket", "s3:GetObject", "s3:ListBucketMultipartUploads"]
WRITE = ["s3:PutObject", "s3:DeleteObject", "s3:AbortMultipartUpload", "s3:ListMultipartUploadParts"]
CAT_READ = ["admin:GetTableCatalog", "admin:GetTableBucket", "admin:GetTableNamespace", "admin:GetTable",
            "admin:GetTableMetadata", "admin:GetTableMetadataLocation"]
CAT_WRITE = ["admin:CreateTable", "admin:CommitTable", "admin:SetTableMetadata", "admin:SetTableMetadataLocation"]
ARNS = [f"arn:aws:s3:::{W}", f"arn:aws:s3:::{W}/*"]


def step(name):
    def wrap(fn):
        t = time.time()
        try:
            results.append(("OK", name, fn(), time.time() - t))
        except Exception as e:
            results.append(("FAIL", name, f"{type(e).__name__}: {str(e)[:300]}", time.time() - t))
        return fn
    return wrap


def outcome(e):
    if isinstance(e, KeyError):
        raise RuntimeError(f"an earlier step failed ({e})")
    s = str(e)
    return "403" if "403" in s else "AccessDenied" if "AccessDenied" in s else s[:160]


def assume(policy):
    sts = boto3.client("sts", endpoint_url=BASE, aws_access_key_id=ISSUER[0], aws_secret_access_key=ISSUER[1],
                       region_name="us-east-1", verify=CA)
    c = sts.assume_role(RoleArn="arn:aws:iam::000000000000:role/qd-job", RoleSessionName="silver-verify",
                        DurationSeconds=900, Policy=json.dumps(policy))["Credentials"]
    return c["AccessKeyId"], c["SecretAccessKey"], c["SessionToken"]


def lake(creds):
    ak, sk, tok = creds
    con = duckdb.connect()
    con.execute("INSTALL avro; LOAD avro; INSTALL iceberg; LOAD iceberg; INSTALL httpfs; LOAD httpfs;")
    con.execute(f"SET ca_cert_file='{CA}'")
    token = f", SESSION_TOKEN '{tok}'" if tok else ""
    con.execute(f"""CREATE SECRET s3 (TYPE s3, KEY_ID '{ak}', SECRET '{sk}'{token},
                    REGION 'us-east-1', ENDPOINT 'localhost', URL_STYLE 'path', USE_SSL true)""")
    con.execute(f"""ATTACH '{W}' AS lake (TYPE iceberg, URI '{BASE}/iceberg',
                    AUTHORIZATION_TYPE 'sigv4', SIGV4_SERVICE 's3', SIGV4_REGION 'us-east-1', SECRET s3)""")
    return con


def policy(writes, catalog=ARNS):
    """Reads the warehouse; writes `writes`; the catalog's actions on `catalog`."""
    return {"Version": "2012-10-17", "Statement": [
        {"Effect": "Allow", "Action": READ, "Resource": ARNS},
        {"Effect": "Allow", "Action": WRITE, "Resource": [f"arn:aws:s3:::{W}/{w}" for w in writes]},
        {"Effect": "Allow", "Action": CAT_READ, "Resource": ARNS},
        {"Effect": "Allow", "Action": CAT_WRITE, "Resource": catalog}]}


def ns_record(ns):
    return f".*/namespaces/{ns}/*"


def prefix_of(con, table):
    path = con.execute(f"SELECT file_path FROM iceberg_metadata({table}) LIMIT 1").fetchone()[0]
    return path.split(f"s3://{W}/", 1)[1].split("/data/", 1)[0]


@step("setup (the platform's part): the namespaces, and a bronze cut of generated rows, as the dev loader")
def _():
    con = lake(LOADER)
    for ns in (BRONZE, SILVER, OTHER):
        con.execute(f"CREATE SCHEMA IF NOT EXISTS lake.{ns}")
    con.execute(f"""CREATE TABLE IF NOT EXISTS lake.{BRONZE}.clients
                    (id INTEGER, document VARCHAR, opened VARCHAR, balance VARCHAR, cut DATE) PARTITIONED BY (cut)""")
    con.execute("BEGIN")
    con.execute(f"DELETE FROM lake.{BRONZE}.clients WHERE cut = DATE '{CUT}'")
    # Documents with leading zeros and spaces, some blank; dates as text, some not a date.
    con.execute(f"""INSERT INTO lake.{BRONZE}.clients
                    SELECT g, CASE WHEN g % 50 = 0 THEN '  ' ELSE ' 00' || lpad(g::text, 8, '0') END,
                           CASE WHEN g % 40 = 0 THEN 'n/a' ELSE (DATE '2020-01-01' + (g % 1000)::INTEGER)::VARCHAR END,
                           (g * 1.5)::VARCHAR, DATE '{CUT}'
                    FROM range(1, 2001) t(g)""")
    con.execute("COMMIT")
    C["loader"] = con
    return f"{con.execute(f'SELECT count(*) FROM lake.{BRONZE}.clients').fetchone()[0]} bronze rows"


SILVER_COLUMNS = "id DOUBLE, document VARCHAR, opened DATE, balance DOUBLE, entity_key VARCHAR, entity_key_ok BOOLEAN, cut DATE"


@step("STS (creating in the silver namespace only): DuckDB creates the silver table, partitioned by cut")
def _():
    con = lake(assume(policy([ns_record(SILVER)])))
    con.execute(f"CREATE TABLE lake.{SILVER}.clients ({SILVER_COLUMNS}) PARTITIONED BY (cut)")
    return f"lake.{SILVER}.clients created"


def replace_silver(con):
    key = "NULLIF(ltrim(regexp_replace(document, '[^0-9]', '', 'g'), '0'), '')"
    con.execute("BEGIN")
    con.execute(f"DELETE FROM lake.{SILVER}.clients WHERE cut = DATE '{CUT}'")
    con.execute(f"""INSERT INTO lake.{SILVER}.clients
                    (id, document, opened, balance, entity_key, entity_key_ok, cut) SELECT TRY_CAST(id AS DOUBLE), NULLIF(trim(document), ''), TRY_CAST(opened AS DATE),
                           TRY_CAST(balance AS DOUBLE), {key}, {key} IS NOT NULL, cut
                    FROM lake.{BRONZE}.clients WHERE cut = DATE '{CUT}'""")
    con.execute("COMMIT")


@step("the silver cut written from bronze's: rows, keys not normalized, dates not read")
def _():
    con = C["loader"]
    C["silver_prefix"] = None
    # The table has no file yet: its prefix comes from its first write, by
    # the loader here; the platform asks the catalog (`table_prefix`).
    replace_silver(con)
    C["silver_prefix"] = prefix_of(con, f"lake.{SILVER}.clients")
    n, bad, nodate = con.execute(f"""SELECT count(*), count(*) FILTER (WHERE NOT entity_key_ok),
                                     count(*) FILTER (WHERE opened IS NULL)
                                     FROM lake.{SILVER}.clients WHERE cut = DATE '{CUT}'""").fetchone()
    return f"{n} rows, {bad} keys not normalized, {nodate} dates not read; files under {C['silver_prefix']}/"


@step("STS limited to the silver table's files: the cut replaced twice, same rows, a snapshot each")
def _():
    con = lake(assume(policy([ns_record(SILVER), f"{C['silver_prefix']}/*"])))
    t, out = f"lake.{SILVER}.clients", []
    for i in range(2):
        before = con.execute(f"SELECT count(*) FROM iceberg_snapshots({t})").fetchone()[0]
        replace_silver(con)
        n = con.execute(f"SELECT count(*) FROM {t} WHERE cut = DATE '{CUT}'").fetchone()[0]
        after = con.execute(f"SELECT count(*) FROM iceberg_snapshots({t})").fetchone()[0]
        out.append(f"run {i + 1}: {n} rows, {after - before} new snapshot(s)")
    C["silver_con"] = con
    return "; ".join(out)


@step("the same credentials cannot write the bronze table")
def _():
    try:
        C["silver_con"].execute(f"DELETE FROM lake.{BRONZE}.clients WHERE cut = DATE '{CUT}'")
        return "WROTE (unexpected)"
    except Exception as e:
        return f"denied ({outcome(e)})"


@step("ALTER TABLE ADD COLUMN on the silver table (a mapping's new field), with the same credentials")
def _():
    try:
        C["silver_con"].execute(f"ALTER TABLE lake.{SILVER}.clients ADD COLUMN segment VARCHAR")
        replace_silver(C["silver_con"])
        cols = C["silver_con"].execute(f"DESCRIBE lake.{SILVER}.clients").fetchall()
        return f"added; {len(cols)} columns"
    except Exception as e:
        outcome(e)
        return f"not supported ({type(e).__name__}: {str(e)[:160]})"


@step("DROP TABLE and CREATE again (a mapping's changed type), as the dev loader")
def _():
    con = C["loader"]
    try:
        con.execute(f"DROP TABLE lake.{SILVER}.clients")
        con.execute(f"CREATE TABLE lake.{SILVER}.clients ({SILVER_COLUMNS}) PARTITIONED BY (cut)")
        replace_silver(con)
        n = con.execute(f"SELECT count(*) FROM lake.{SILVER}.clients").fetchone()[0]
        return f"recreated, {n} rows"
    except Exception as e:
        return f"refused ({type(e).__name__}: {str(e)[:160]})"


SCOPES = {
    "the namespace's record": lambda ns: [f"arn:aws:s3:::{W}/.*/namespaces/{ns}/*"],
    "the namespace's name": lambda ns: [f"arn:aws:s3:::{W}/{ns}", f"arn:aws:s3:::{W}/{ns}/*"],
    "a table's ARN": lambda ns: [f"arn:aws:s3:::{W}/.*/namespaces/{ns}/tables/*"],
}


@step("admin:CreateTable scoped to one namespace: created there, refused in another (S3 writes on both)")
def _():
    out = []
    for label, scope in SCOPES.items():
        con = lake(assume(policy([ns_record(SILVER), ns_record(OTHER)], catalog=scope(SILVER))))
        t = f"scoped_{int(time.time() * 1000) % 10**8}"
        try:
            con.execute(f"CREATE TABLE lake.{SILVER}.{t} (x INTEGER, cut DATE) PARTITIONED BY (cut)")
            here = "created"
        except Exception as e:
            here = f"refused ({outcome(e)})"
        try:
            con.execute(f"CREATE TABLE lake.{OTHER}.{t} (x INTEGER, cut DATE) PARTITIONED BY (cut)")
            there = "CREATED in the other"
        except Exception as e:
            there = f"refused in the other ({outcome(e)})"
        out.append(f"{label}: {here}, {there}")
    return "; ".join(out)


@step("admin:CommitTable scoped to the silver namespace: replaces the silver cut")
def _():
    out = []
    for label, scope in SCOPES.items():
        con = lake(assume(policy([ns_record(SILVER), f"{prefix_of(C['loader'], f'lake.{SILVER}.clients')}/*"],
                                 catalog=scope(SILVER))))
        try:
            replace_silver(con)
            out.append(f"{label}: replaced")
        except Exception as e:
            out.append(f"{label}: refused ({outcome(e)})")
    return "; ".join(out)


@step("cleanup: drop the verify tables and namespaces (STS with admin:DeleteTable on the warehouse)")
def _():
    con = lake(assume({"Version": "2012-10-17", "Statement": [
        {"Effect": "Allow", "Action": ["s3:*"], "Resource": ARNS},
        {"Effect": "Allow", "Action": CAT_READ + ["admin:DeleteTable", "admin:DeleteTableNamespace"],
         "Resource": ARNS}]}))
    out = []
    for ns in (BRONZE, SILVER, OTHER):
        for (t,) in con.execute("SELECT table_name FROM duckdb_tables() WHERE database_name = 'lake' "
                                "AND schema_name = ?", [ns]).fetchall():
            try:
                con.execute(f"DROP TABLE lake.{ns}.{t}")
                out.append(f"{ns}.{t} dropped")
            except Exception as e:
                out.append(f"{ns}.{t}: {outcome(e)}")
        try:
            con.execute(f"DROP SCHEMA lake.{ns}")
        except Exception as e:
            out.append(f"{ns}: {outcome(e)}")
    return "; ".join(out)


for status, name, detail, secs in results:
    print(f"[{status:4}] {name} ({secs:.1f}s): {detail}")
