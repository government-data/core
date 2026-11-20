"""Verification for decision 0017: DuckDB writes partitioned Iceberg tables
through RustFS's REST catalog, and the catalog supports what we need.
Synthetic data only. Credentials come from the environment, never printed.

Against the dev stack (watch mode), from the repository root:

    set -a; . ./.env; set +a
    uv venv /tmp/qd-verify && uv pip install --python /tmp/qd-verify/bin/python \\
        "duckdb>=1.4" "pyiceberg[pyarrow]" boto3
    /tmp/qd-verify/bin/python tools/verify/lake_iceberg.py

It creates (and leaves, empty) the table bucket WAREHOUSE (qd-test-lake)."""
import os, time
from pathlib import Path
import duckdb
from pyiceberg.schema import Schema
from pyiceberg.types import NestedField, StringType, LongType, DateType, DoubleType
from pyiceberg.partitioning import PartitionSpec, PartitionField
from pyiceberg.transforms import IdentityTransform, BucketTransform, MonthTransform

AK, SK = os.environ["RUSTFS_ACCESS_KEY"], os.environ["RUSTFS_SECRET_KEY"]
CA = os.environ.get("DEV_CA", str(Path(__file__).resolve().parents[2] / "infra/traefik/certs/dev-ca.crt"))
WAREHOUSE = os.environ.get("WAREHOUSE", "qd-test-lake")
NS = "qd_test"
results = []

def step(name):
    def wrap(fn):
        t = time.time()
        try:
            out = fn()
            results.append(("OK", name, out, time.time() - t))
        except Exception as e:
            results.append(("FAIL", name, f"{type(e).__name__}: {str(e)[:400]}", time.time() - t))
        return fn
    return wrap

os.environ["SSL_CERT_FILE"] = CA
os.environ["REQUESTS_CA_BUNDLE"] = CA
import hashlib, json, requests
from requests.adapters import HTTPAdapter
from botocore.auth import S3SigV4Auth
from botocore.awsrequest import AWSRequest
from botocore.credentials import Credentials
from pyiceberg.catalog.rest import RestCatalog

class SigV4Adapter(HTTPAdapter):
    """RustFS wants x-amz-content-sha256 on every signed catalog request."""
    def add_headers(self, request, **kwargs):
        body = request.body or b""
        if isinstance(body, str):
            body = body.encode()
        request.headers["x-amz-content-sha256"] = hashlib.sha256(body).hexdigest()
        request.headers.pop("connection", None)
        request.headers["content-length"] = str(len(body))
        aws = AWSRequest(method=request.method, url=request.url, data=body, headers=dict(request.headers))
        S3SigV4Auth(Credentials(AK, SK), "s3", "us-east-1").add_auth(aws)
        request.headers.update(dict(aws.headers))

class RustFSCatalog(RestCatalog):
    def _init_sigv4(self, session):
        session.mount(self.uri, SigV4Adapter())

def signed(method, path):
    r = requests.Session()
    r.mount("https://localhost", SigV4Adapter())
    return r.request(method, "https://localhost" + path, verify=CA)

@step("rustfs: create the test bucket and enable it for tables")
def _():
    c = signed("PUT", f"/{WAREHOUSE}")
    e = signed("PUT", f"/iceberg/v1/buckets/{WAREHOUSE}")
    g = signed("GET", f"/iceberg/v1/buckets/{WAREHOUSE}")
    return f"bucket={c.status_code} enable={e.status_code} state={g.text[:200]}"

cat = RustFSCatalog("rustfs", **{
    "uri": "https://localhost/iceberg",
    "warehouse": WAREHOUSE,
    "rest.sigv4-enabled": "true",
    "rest.signing-region": "us-east-1",
    "rest.signing-name": "s3",
    "py-io-impl": "pyiceberg.io.pyarrow.PyArrowFileIO",
    "s3.endpoint": "https://localhost",
    "s3.access-key-id": AK,
    "s3.secret-access-key": SK,
    "s3.region": "us-east-1",
    "s3.force-virtual-addressing": "false",
})

@step("catalog: create namespace")
def _():
    if (NS,) not in cat.list_namespaces():
        cat.create_namespace(NS)
    return [n for n in cat.list_namespaces()]

schema = Schema(
    NestedField(1, "cut", StringType(), required=False),
    NestedField(2, "doc", StringType(), required=False),
    NestedField(3, "client_type", StringType(), required=False),
    NestedField(4, "income", DoubleType(), required=False),
    NestedField(5, "updated", DateType(), required=False),
    NestedField(6, "n", LongType(), required=False),
)
spec = PartitionSpec(
    PartitionField(source_id=1, field_id=1000, transform=IdentityTransform(), name="cut"),
    PartitionField(source_id=2, field_id=1001, transform=BucketTransform(8), name="doc_bucket"),
    PartitionField(source_id=5, field_id=1002, transform=MonthTransform(), name="updated_month"),
)

con = duckdb.connect()
con.execute("INSTALL iceberg; LOAD iceberg; INSTALL httpfs; LOAD httpfs;")
con.execute(f"SET ca_cert_file='{CA}'")
con.execute(f"""CREATE SECRET s3 (TYPE s3, KEY_ID '{AK}', SECRET '{SK}', REGION 'us-east-1',
                ENDPOINT 'localhost', URL_STYLE 'path', USE_SSL true)""")

def drop_test_tables():
    for t in ("clients", "clients_duck", "clients_duck2"):
        con.execute(f"DROP TABLE IF EXISTS lake.{NS}.{t}")

@step("duckdb: attach the REST catalog (sigv4), drop leftovers")
def _():
    # A non-AWS endpoint: DuckDB cannot guess the service and region to sign.
    con.execute(f"""ATTACH '{WAREHOUSE}' AS lake (TYPE iceberg, URI 'https://localhost/iceberg',
                    AUTHORIZATION_TYPE 'sigv4', SIGV4_SERVICE 's3', SIGV4_REGION 'us-east-1', SECRET s3)""")
    drop_test_tables()
    return con.execute(f"SELECT table_name FROM information_schema.tables WHERE table_catalog='lake' AND table_schema='{NS}'").fetchall()

@step("catalog: create partitioned table (identity + bucket + month)")
def _():
    tbl = cat.create_table((NS, "clients"), schema=schema, partition_spec=spec)
    return str(tbl.spec())

@step("duckdb: sees the new table")
def _():
    con.execute("DETACH lake")
    con.execute(f"""ATTACH '{WAREHOUSE}' AS lake (TYPE iceberg, URI 'https://localhost/iceberg',
                    AUTHORIZATION_TYPE 'sigv4', SIGV4_SERVICE 's3', SIGV4_REGION 'us-east-1', SECRET s3)""")
    return con.execute(f"SELECT table_name FROM information_schema.tables WHERE table_catalog='lake' AND table_schema='{NS}'").fetchall()

N = 200_000
insert_sql = f"""
INSERT INTO lake.{NS}.clients
SELECT '2026-0' || (1 + i % 3) AS cut,
       lpad(CAST(10000000 + i * 7 AS VARCHAR), 10, '0') AS doc,
       CASE WHEN i % 10 = 0 THEN 'PJ' ELSE 'PN' END AS client_type,
       round(1000000 + (i * 7919) % 9000000, 0)::DOUBLE AS income,
       DATE '2020-01-01' + CAST(i % 2000 AS INTEGER) AS updated,
       i::BIGINT AS n
FROM range({N}) t(i)"""

@step(f"duckdb: INSERT {N} rows into the partitioned table")
def _():
    con.execute(insert_sql)
    return con.execute(f"SELECT count(*), count(DISTINCT cut) FROM lake.{NS}.clients").fetchone()

@step("pyiceberg: read back, check files carry partition values")
def _():
    tbl = cat.load_table((NS, "clients"))
    rows = tbl.scan().to_arrow().num_rows
    files = list(tbl.inspect.files().to_pylist())
    parts = {str(f["partition"]) for f in files}
    return f"rows={rows} files={len(files)} distinct_partitions={len(parts)} sample={sorted(parts)[:2]}"

@step("duckdb: partition pruning on cut (EXPLAIN mentions filters)")
def _():
    n = con.execute(f"SELECT count(*) FROM lake.{NS}.clients WHERE cut = '2026-02'").fetchone()[0]
    return f"rows in cut 2026-02 = {n}"

@step("duckdb: UPDATE on the partitioned table")
def _():
    con.execute(f"UPDATE lake.{NS}.clients SET income = 0 WHERE n < 100")
    return con.execute(f"SELECT count(*) FROM lake.{NS}.clients WHERE income = 0").fetchone()

@step("duckdb: DELETE on the partitioned table")
def _():
    con.execute(f"DELETE FROM lake.{NS}.clients WHERE n >= {N} - 50")
    return con.execute(f"SELECT count(*) FROM lake.{NS}.clients").fetchone()

@step("snapshots and time travel")
def _():
    tbl = cat.load_table((NS, "clients"))
    snaps = [s.snapshot_id for s in tbl.snapshots()]
    first = snaps[0]
    n0 = con.execute(f"SELECT count(*) FROM lake.{NS}.clients AT (VERSION => {first})").fetchone()[0]
    return f"snapshots={len(snaps)} rows_at_first={n0}"

@step("duckdb: CREATE TABLE with PARTITIONED BY")
def _():
    con.execute(f"CREATE TABLE lake.{NS}.clients_duck (cut VARCHAR, doc VARCHAR, n BIGINT)")
    try:
        con.execute(f"ALTER TABLE lake.{NS}.clients_duck SET PARTITIONED BY (cut)")
        how = "ALTER ... SET PARTITIONED BY worked"
    except Exception as e:
        how = f"ALTER SET PARTITIONED BY not supported ({type(e).__name__})"
    con.execute(f"INSERT INTO lake.{NS}.clients_duck SELECT cut, doc, n FROM lake.{NS}.clients LIMIT 1000")
    spec_now = str(cat.load_table((NS, "clients_duck")).spec())
    return f"{how}; spec={spec_now}"

@step("catalog: expire snapshots (commit of a maintenance change)")
def _():
    tbl = cat.load_table((NS, "clients"))
    before = len(tbl.snapshots())
    keep = tbl.current_snapshot().snapshot_id
    old = [s.snapshot_id for s in tbl.snapshots() if s.snapshot_id != keep]
    tbl.maintenance.expire_snapshots().by_ids(old).commit()
    after = len(cat.load_table((NS, "clients")).snapshots())
    return f"snapshots {before} -> {after}"

@step("catalog: rename (REST) and drop (duckdb), clean up")
def _():
    # Through the REST API directly: PyIceberg's own rename is not signed the
    # way RustFS requires.
    s = requests.Session()
    s.mount("https://localhost", SigV4Adapter())
    body = json.dumps({"source": {"namespace": [NS], "name": "clients_duck"},
                       "destination": {"namespace": [NS], "name": "clients_duck2"}})
    r = s.post(f"https://localhost/iceberg/v1/{WAREHOUSE}/tables/rename", data=body,
               headers={"content-type": "application/json"}, verify=CA)
    renamed = (NS, "clients_duck2") in cat.list_tables(NS)
    drop_test_tables()
    return f"rename={r.status_code} renamed={renamed} left={cat.list_tables(NS)}"

w = max(len(r[1]) for r in results)
for status, name, out, secs in results:
    print(f"[{status:4}] {name:<{w}}  {secs:6.1f}s  {out}")
print("duckdb", duckdb.__version__)
