# /// script
# requires-python = ">=3.11"
# dependencies = ["duckdb>=1.4", "boto3"]
# ///
"""Verification for decisions 0017 and 0030: a job reads its tenant's lake
through DuckDB's `iceberg` extension with credentials of its own, issued by
STS AssumeRole for the platform's issuer (infra/rustfs/sts-issuer.sh) and
narrowed by a read-only session policy to one warehouse. Counts only; no
value is read, credentials are never printed.

From the repository root, with the dev stack up and a cut loaded
(tools/dev/lake-load.sh):

    uv run tools/verify/lake_sts_duckdb.py
"""
import json, os, time
from pathlib import Path

import boto3, duckdb

ROOT = Path(__file__).resolve().parents[2]
env = dict(l.split("=", 1) for l in (ROOT / ".env").read_text().splitlines() if "=" in l and not l.startswith("#"))
ISSUER = (env["RUSTFS_STS_ISSUER_ACCESS_KEY"], env["RUSTFS_STS_ISSUER_SECRET_KEY"])
CA = str(ROOT / "infra/traefik/certs/dev-ca.crt")
BASE = "https://localhost"
WAREHOUSE, OTHER = "qd-tenant-00000000000000000000000000000001", "qd-tenant-other"
TABLE = "clientes_naturales_bronze.clientes_naturales"
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


def read_only(warehouse):
    arns = [f"arn:aws:s3:::{warehouse}", f"arn:aws:s3:::{warehouse}/*"]
    return {"Version": "2012-10-17", "Statement": [
        {"Effect": "Allow", "Action": ["s3:GetBucketLocation", "s3:ListBucket", "s3:GetObject"], "Resource": arns},
        {"Effect": "Allow", "Action": ["admin:GetTableCatalog", "admin:GetTableBucket", "admin:GetTableNamespace",
                                       "admin:GetTable", "admin:GetTableMetadata", "admin:GetTableMetadataLocation"],
         "Resource": arns}]}


def assume(policy):
    sts = boto3.client("sts", endpoint_url=BASE, aws_access_key_id=ISSUER[0], aws_secret_access_key=ISSUER[1],
                       region_name="us-east-1", verify=CA)
    c = sts.assume_role(RoleArn="arn:aws:iam::000000000000:role/qd-job", RoleSessionName="job-verify",
                        DurationSeconds=900, Policy=json.dumps(policy))["Credentials"]
    return c["AccessKeyId"], c["SecretAccessKey"], c["SessionToken"]


def duck(creds, warehouse):
    con = duckdb.connect()
    con.execute("INSTALL iceberg; LOAD iceberg; INSTALL httpfs; LOAD httpfs;")
    con.execute(f"SET ca_cert_file='{CA}'")
    ak, sk, tok = creds
    con.execute(f"""CREATE SECRET s3 (TYPE s3, KEY_ID '{ak}', SECRET '{sk}', SESSION_TOKEN '{tok}',
                    REGION 'us-east-1', ENDPOINT 'localhost', URL_STYLE 'path', USE_SSL true)""")
    con.execute(f"""ATTACH '{warehouse}' AS lake (TYPE iceberg, URI '{BASE}/iceberg',
                    AUTHORIZATION_TYPE 'sigv4', SIGV4_SERVICE 's3', SIGV4_REGION 'us-east-1', SECRET s3)""")
    return con


C = {}


@step("AssumeRole: 15 minutes, read-only on qd-tenant-00000000000000000000000000000001")
def _():
    C["creds"] = assume(read_only(WAREHOUSE))
    return "issued, with a session token"


@step("DuckDB attaches the warehouse with them and counts a bronze table")
def _():
    C["con"] = duck(C["creds"], WAREHOUSE)
    return f"{C['con'].execute(f'SELECT count(*) FROM lake.{TABLE}').fetchone()[0]} rows"


@step("DuckDB: writing a table is denied")
def _():
    try:
        C["con"].execute("CREATE TABLE lake.clientes_naturales_bronze.sts_check AS SELECT 1 AS x")
        return "NOT DENIED (unexpected)"
    except Exception as e:
        return f"denied ({type(e).__name__}, {'403' if '403' in str(e) else 'AccessDenied' if 'AccessDenied' in str(e) else str(e)[:80]})"


@step("DuckDB: credentials for another tenant's warehouse cannot read this one")
def _():
    try:
        con = duck(assume(read_only(OTHER)), WAREHOUSE)
        return f"NOT DENIED (unexpected): {con.execute(f'SELECT count(*) FROM lake.{TABLE}').fetchone()[0]}"
    except Exception as e:
        return f"denied ({type(e).__name__}, {'403' if '403' in str(e) else 'AccessDenied' if 'AccessDenied' in str(e) else str(e)[:80]})"


@step("the issuer cannot grant beyond its ceiling (a bucket not qd-tenant-*)")
def _():
    creds = assume(read_only("qd-not-a-tenant"))
    s3 = boto3.client("s3", endpoint_url=BASE, aws_access_key_id=creds[0], aws_secret_access_key=creds[1],
                      aws_session_token=creds[2], region_name="us-east-1", verify=CA)
    try:
        s3.list_objects_v2(Bucket="qd-not-a-tenant")
        return "NOT DENIED (unexpected)"
    except Exception as e:
        return f"denied ({type(e).__name__}, {'403' if '403' in str(e) else 'AccessDenied' if 'AccessDenied' in str(e) else str(e)[:80]})"


for status, name, detail, secs in results:
    print(f"[{status:4}] {name} ({secs:.1f}s): {detail}")
