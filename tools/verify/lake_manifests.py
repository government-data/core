# /// script
# requires-python = ">=3.11"
# dependencies = ["duckdb==2.0.0", "boto3"]
# ///
"""Verification for decision 0035: what an evaluation's manifest needs from
the lake.

1. Which Iceberg snapshot a job read, as DuckDB (the worker's engine, the
   same version the worker bundles) sees it through the attached catalog,
   and reading that snapshot again by its id (a run repeated from its
   manifest).
2. Credentials from STS whose session policy allows `PutObject` under
   `manifests/` only: they write a manifest, and cannot write elsewhere in
   the warehouse, overwrite nothing outside, or read the tables.

Counts and ids only; no value is read, credentials are never printed. From
the repository root, with the dev stack up and a cut loaded
(tools/dev/lake-load.sh):

    uv run tools/verify/lake_manifests.py
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
NS, TABLE = "clientes_juridicas_bronze", "clientes_juridicas"
KEY = "manifests/verify/2026-10-31/evaluate/verify.json"
results, C = [], {}


def step(name):
    def wrap(fn):
        t = time.time()
        try:
            results.append(("OK", name, fn(), time.time() - t))
        except Exception as e:
            results.append(("FAIL", name, f"{type(e).__name__}: {str(e)[:300]}", time.time() - t))
        return fn
    return wrap


def denied(fn):
    try:
        fn()
        return "NOT DENIED (unexpected)"
    except Exception as e:
        s = str(e)
        return f"denied ({type(e).__name__}, {'403' if '403' in s else 'AccessDenied' if 'AccessDenied' in s else s[:80]})"


def assume(policy):
    sts = boto3.client("sts", endpoint_url=BASE, aws_access_key_id=ISSUER[0], aws_secret_access_key=ISSUER[1],
                       region_name="us-east-1", verify=CA)
    c = sts.assume_role(RoleArn="arn:aws:iam::000000000000:role/qd-job", RoleSessionName="manifest-verify",
                        DurationSeconds=900, Policy=json.dumps(policy))["Credentials"]
    return c["AccessKeyId"], c["SecretAccessKey"], c["SessionToken"]


def read_only(w):
    arns = [f"arn:aws:s3:::{w}", f"arn:aws:s3:::{w}/*"]
    return {"Version": "2012-10-17", "Statement": [
        {"Effect": "Allow", "Action": ["s3:GetBucketLocation", "s3:ListBucket", "s3:GetObject"], "Resource": arns},
        {"Effect": "Allow", "Action": ["admin:GetTableCatalog", "admin:GetTableBucket", "admin:GetTableNamespace",
                                       "admin:GetTable", "admin:GetTableMetadata", "admin:GetTableMetadataLocation"],
         "Resource": arns}]}


def manifests_only(w):
    return {"Version": "2012-10-17", "Statement": [
        {"Effect": "Allow", "Action": ["s3:PutObject"], "Resource": [f"arn:aws:s3:::{w}/manifests/*"]}]}


def s3(creds):
    return boto3.client("s3", endpoint_url=BASE, aws_access_key_id=creds[0], aws_secret_access_key=creds[1],
                        aws_session_token=creds[2], region_name="us-east-1", verify=CA)


@step("DuckDB version")
def _():
    return duckdb.__version__


@step("DuckDB attaches the warehouse read-only")
def _():
    ak, sk, tok = assume(read_only(WAREHOUSE))
    con = duckdb.connect()
    con.execute("INSTALL avro; LOAD avro; INSTALL iceberg; LOAD iceberg; INSTALL httpfs; LOAD httpfs;")
    con.execute(f"SET ca_cert_file='{CA}'")
    con.execute(f"""CREATE SECRET s3 (TYPE s3, KEY_ID '{ak}', SECRET '{sk}', SESSION_TOKEN '{tok}',
                    REGION 'us-east-1', ENDPOINT 'localhost', URL_STYLE 'path', USE_SSL true)""")
    con.execute(f"""ATTACH '{WAREHOUSE}' AS lake (TYPE iceberg, URI '{BASE}/iceberg',
                    AUTHORIZATION_TYPE 'sigv4', SIGV4_SERVICE 's3', SIGV4_REGION 'us-east-1', SECRET s3)""")
    C["con"] = con
    return "attached"


@step("iceberg_snapshots() on the attached table: the current snapshot's id")
def _():
    rows = C["con"].execute(f"SELECT * FROM iceberg_snapshots(lake.{NS}.{TABLE})").fetchall()
    cols = [d[0] for d in C["con"].description]
    C["snap"] = rows[-1][cols.index("snapshot_id")]
    return f"{len(rows)} snapshots, columns {cols}, latest id {C['snap']}"


@step("iceberg_metadata() on the attached table")
def _():
    rows = C["con"].execute(f"SELECT count(*) FROM iceberg_metadata(lake.{NS}.{TABLE})").fetchone()
    return f"{rows[0]} manifest entries"


@step("reading that snapshot by id: AT (VERSION => id) gives the same count")
def _():
    now = C["con"].execute(f"SELECT count(*) FROM lake.{NS}.{TABLE}").fetchone()[0]
    at = C["con"].execute(f"SELECT count(*) FROM lake.{NS}.{TABLE} AT (VERSION => {C['snap']})").fetchone()[0]
    return f"current {now}, at snapshot {at}, equal {now == at}"


@step("STS: PutObject under manifests/ only — writes a manifest")
def _():
    C["w"] = s3(assume(manifests_only(WAREHOUSE)))
    C["w"].put_object(Bucket=WAREHOUSE, Key=KEY, Body=b'{"verify": true}', ContentType="application/json")
    return f"wrote {KEY}"


@step("STS: the same credentials cannot write outside manifests/")
def _():
    return denied(lambda: C["w"].put_object(Bucket=WAREHOUSE, Key="verify-outside.json", Body=b"{}"))


@step("STS: the same credentials cannot read the warehouse")
def _():
    return denied(lambda: C["w"].get_object(Bucket=WAREHOUSE, Key=KEY))


@step("STS: PutObject with If-None-Match: * refuses to overwrite an existing manifest")
def _():
    try:
        C["w"].put_object(Bucket=WAREHOUSE, Key=KEY, Body=b'{"verify": false}', IfNoneMatch="*")
        return "OVERWRITTEN (If-None-Match not honoured)"
    except Exception as e:
        s = str(e)
        return f"refused ({type(e).__name__}, {'412' if '412' in s or 'PreconditionFailed' in s else s[:100]})"


for status, name, detail, secs in results:
    print(f"[{status:4}] {name} ({secs:.1f}s): {detail}")
