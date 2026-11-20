# /// script
# requires-python = ">=3.11"
# dependencies = ["duckdb==2.0.0", "boto3", "requests"]
# ///
"""Verification for a `maintain` job (decision 0017, point 4): what keeping
an Iceberg table small and clean takes, with DuckDB (the worker's engine,
the version it bundles) and the catalog's REST API, on a synthetic table
only (`verify_maint_bronze.events`).

1. Compaction: DuckDB's `iceberg_rewrite_data_files` on a table of many
   small files; the files after, and the same rows and sums.
2. Snapshot expiry: a `remove-snapshots` commit through the REST catalog
   (DuckDB has none); the snapshots after, whether the table still reads,
   and whether RustFS removes any file by itself.
3. Orphan files: the data files no kept snapshot's manifests name, in any
   entry (`iceberg_metadata(…, snapshot_from_id => …)` for each, its
   `DELETED` entries too), deleted; the table still reads the same and
   still takes a write. A file a manifest still names as `DELETED` is not
   an orphan: RustFS refuses a later commit if it is gone.

Counts and names only. From the repository root, with the dev stack up and
the dev loader's keys in .env (tools/dev/lake-load.sh):

    uv run tools/verify/lake_maintain.py
"""
import hashlib, json, time
from pathlib import Path

import boto3, duckdb, requests
from botocore.auth import SigV4Auth
from botocore.awsrequest import AWSRequest
from botocore.credentials import Credentials

ROOT = Path(__file__).resolve().parents[2]
env = dict(l.split("=", 1) for l in (ROOT / ".env").read_text().splitlines() if "=" in l and not l.startswith("#"))
AK, SK = env["LAKE_LOADER_ACCESS_KEY"], env["LAKE_LOADER_SECRET_KEY"]
CA = str(ROOT / "infra/traefik/certs/dev-ca.crt")
BASE = "https://localhost"
W = "qd-tenant-00000000000000000000000000000001"
NS, T = "verify_maint_bronze", f"events_{int(time.time())}"
TABLE = f"lake.{NS}.{T}"
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


def rest(method, path, body=None):
    url = f"{BASE}/iceberg/v1/{W}{path}"
    data = json.dumps(body).encode() if body is not None else b""
    h = {"x-amz-content-sha256": hashlib.sha256(data).hexdigest(), "content-type": "application/json"}
    r = AWSRequest(method=method, url=url, data=data, headers=h)
    SigV4Auth(Credentials(AK, SK), "s3", "us-east-1").add_auth(r)
    return requests.request(method, url, data=data, headers=dict(r.headers), verify=CA)


s3 = boto3.client("s3", endpoint_url=BASE, aws_access_key_id=AK, aws_secret_access_key=SK,
                  region_name="us-east-1", verify=CA)


def objects(prefix):
    keys, token = [], None
    while True:
        kw = {"Bucket": W, "Prefix": prefix}
        if token:
            kw["ContinuationToken"] = token
        page = s3.list_objects_v2(**kw)
        keys += [o["Key"] for o in page.get("Contents", [])]
        if not page.get("IsTruncated"):
            return keys
        token = page["NextContinuationToken"]


def totals(con):
    return con.execute(f"SELECT count(*), sum(amount) FROM {TABLE}").fetchone()


def live_files(con, snapshot=None):
    arg = f", snapshot_from_id => {snapshot}" if snapshot else ""
    rows = con.execute(f"SELECT file_path FROM iceberg_metadata({TABLE}{arg}) "
                       "WHERE manifest_content = 'DATA' AND status != 'DELETED'").fetchall()
    return {p for (p,) in rows}


def named_files(con, snapshot):
    """Every data file a snapshot's manifests name, `DELETED` entries too."""
    rows = con.execute(f"SELECT file_path FROM iceberg_metadata({TABLE}, snapshot_from_id => {snapshot}) "
                       "WHERE manifest_content = 'DATA'").fetchall()
    return {p for (p,) in rows}


@step("setup: a table of 20 small inserts (20 files, 20 snapshots)")
def _():
    con = duckdb.connect()
    con.execute("INSTALL avro; LOAD avro; INSTALL iceberg; LOAD iceberg; INSTALL httpfs; LOAD httpfs;")
    con.execute(f"SET ca_cert_file='{CA}'")
    con.execute(f"""CREATE SECRET s3 (TYPE s3, KEY_ID '{AK}', SECRET '{SK}', REGION 'us-east-1',
                    ENDPOINT 'localhost', URL_STYLE 'path', USE_SSL true)""")
    con.execute(f"""ATTACH '{W}' AS lake (TYPE iceberg, URI '{BASE}/iceberg',
                    AUTHORIZATION_TYPE 'sigv4', SIGV4_SERVICE 's3', SIGV4_REGION 'us-east-1', SECRET s3)""")
    con.execute(f"CREATE SCHEMA IF NOT EXISTS lake.{NS}")
    con.execute(f"CREATE TABLE {TABLE} (id INTEGER, amount DECIMAL(12,2), cut DATE) PARTITIONED BY (cut)")
    for i in range(20):
        con.execute(f"INSERT INTO {TABLE} SELECT g, g * 1.5, DATE '2026-10-31' FROM range({i * 100}, {(i + 1) * 100}) t(g)")
    C["con"] = con
    C["totals"] = totals(con)
    loc = rest("GET", f"/namespaces/{NS}/tables/{T}").json()["metadata"]["location"]
    C["prefix"] = loc.split(f"s3://{W}/", 1)[1]
    snaps = con.execute(f"SELECT count(*) FROM iceberg_snapshots({TABLE})").fetchone()[0]
    return f"{len(live_files(con))} live files, {snaps} snapshots, totals {C['totals']}"


@step("compaction: iceberg_rewrite_data_files")
def _():
    con = C["con"]
    out = con.execute(f"SELECT * FROM iceberg_rewrite_data_files('{TABLE}', min_input_files => 2)").fetchall()
    cols = [d[0] for d in con.description]
    same = totals(con) == C["totals"]
    return f"{dict(zip(cols, out[0])) if out else out}; live files {len(live_files(con))}; same rows and sums {same}"


@step("expiry: remove-snapshots through the REST catalog, keeping the current one")
def _():
    con = C["con"]
    snaps = [s for (s,) in con.execute(f"SELECT snapshot_id FROM iceberg_snapshots({TABLE}) ORDER BY sequence_number").fetchall()]
    C["old"], current = snaps[:-1], snaps[-1]
    before = len(objects(f"{C['prefix']}/data/"))
    r = rest("POST", f"/namespaces/{NS}/tables/{T}", {
        "requirements": [{"type": "assert-ref-snapshot-id", "ref": "main", "snapshot-id": current}],
        "updates": [{"action": "remove-snapshots", "snapshot-ids": C["old"]}],
    })
    con.execute("DETACH lake")
    con.execute(f"""ATTACH '{W}' AS lake (TYPE iceberg, URI '{BASE}/iceberg',
                    AUTHORIZATION_TYPE 'sigv4', SIGV4_SERVICE 's3', SIGV4_REGION 'us-east-1', SECRET s3)""")
    left = con.execute(f"SELECT count(*) FROM iceberg_snapshots({TABLE})").fetchone()[0]
    after = len(objects(f"{C['prefix']}/data/"))
    same = totals(con) == C["totals"]
    return (f"commit {r.status_code}; snapshots left {left}; data objects {before} -> {after} "
            f"(RustFS removes none by itself: {before == after}); same rows and sums {same}")


@step("time travel to an expired snapshot")
def _():
    try:
        C["con"].execute(f"SELECT count(*) FROM {TABLE} AT (VERSION => {C['old'][0]})").fetchone()
        return "STILL READS (unexpected)"
    except Exception as e:
        return f"refused ({type(e).__name__})"


@step("orphans: data files no kept snapshot's manifests name, deleted; it reads and takes a write")
def _():
    con = C["con"]
    kept = {s for (s,) in con.execute(f"SELECT snapshot_id FROM iceberg_snapshots({TABLE})").fetchall()}
    referenced = set()
    for snap in kept:
        referenced |= named_files(con, snap)
    stored = objects(f"{C['prefix']}/data/")
    orphans = [k for k in stored if f"s3://{W}/{k}" not in referenced]
    for k in orphans:
        s3.delete_object(Bucket=W, Key=k)
    con.execute("DETACH lake")
    con.execute(f"""ATTACH '{W}' AS lake (TYPE iceberg, URI '{BASE}/iceberg',
                    AUTHORIZATION_TYPE 'sigv4', SIGV4_SERVICE 's3', SIGV4_REGION 'us-east-1', SECRET s3)""")
    same = totals(con) == C["totals"]
    con.execute(f"INSERT INTO {TABLE} VALUES (999999, 1.00, DATE '2026-10-31')")
    return (f"{len(stored)} data objects, {len(referenced)} named by {len(kept)} kept snapshot(s), "
            f"{len(orphans)} orphans deleted; same rows and sums {same}; a write after it commits")


@step("a later pass: after a commit and another expiry, the replaced files are orphans")
def _():
    con = C["con"]
    snaps = [s for (s,) in con.execute(f"SELECT snapshot_id FROM iceberg_snapshots({TABLE}) ORDER BY sequence_number").fetchall()]
    r = rest("POST", f"/namespaces/{NS}/tables/{T}", {
        "requirements": [{"type": "assert-ref-snapshot-id", "ref": "main", "snapshot-id": snaps[-1]}],
        "updates": [{"action": "remove-snapshots", "snapshot-ids": snaps[:-1]}],
    })
    con.execute("DETACH lake")
    con.execute(f"""ATTACH '{W}' AS lake (TYPE iceberg, URI '{BASE}/iceberg',
                    AUTHORIZATION_TYPE 'sigv4', SIGV4_SERVICE 's3', SIGV4_REGION 'us-east-1', SECRET s3)""")
    kept = {s for (s,) in con.execute(f"SELECT snapshot_id FROM iceberg_snapshots({TABLE})").fetchall()}
    named = set()
    for snap in kept:
        named |= named_files(con, snap)
    stored = objects(f"{C['prefix']}/data/")
    orphans = [k for k in stored if f"s3://{W}/{k}" not in named]
    for k in orphans:
        s3.delete_object(Bucket=W, Key=k)
    con.execute("DETACH lake")
    con.execute(f"""ATTACH '{W}' AS lake (TYPE iceberg, URI '{BASE}/iceberg',
                    AUTHORIZATION_TYPE 'sigv4', SIGV4_SERVICE 's3', SIGV4_REGION 'us-east-1', SECRET s3)""")
    rows = con.execute(f"SELECT count(*) FROM {TABLE}").fetchone()[0]
    con.execute(f"INSERT INTO {TABLE} VALUES (999998, 1.00, DATE '2026-10-31')")
    return (f"expiry {r.status_code}; {len(stored)} data objects, {len(named)} named, {len(orphans)} orphans "
            f"deleted; {rows} rows (2001 expected); a write after it commits")


for status, name, detail, secs in results:
    print(f"[{status:4}] {name} ({secs:.1f}s): {detail}")
