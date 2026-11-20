# /// script
# requires-python = ">=3.11"
# dependencies = ["pyiceberg[pyarrow]>=0.9", "requests", "botocore", "boto3", "pyyaml"]
# ///
"""Verification for decision 0055: deleting a table of the lake through the
Iceberg REST catalog RustFS serves. A throwaway namespace and table with
synthetic rows are written, their objects in the warehouse counted, then
the table dropped (asked to purge, then without), the objects counted
again and deleted under its prefix; the namespace is then dropped.
Counts only.

From the repository root, with the dev stack up:

    set -a; . ./.env; set +a; uv run tools/verify/lake_drop.py
"""
import os, sys, time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/dev"))
from lake_load import CA, catalog  # noqa: E402

import boto3  # noqa: E402
import pyarrow as pa  # noqa: E402

WAREHOUSE = os.environ.get("TRINO_LAKE_WAREHOUSE", "qd-tenant-00000000000000000000000000000001")
NS = "verify_drop_bronze"
s3 = boto3.client("s3", endpoint_url="https://localhost", aws_access_key_id=os.environ["LAKE_LOADER_ACCESS_KEY"],
                  aws_secret_access_key=os.environ["LAKE_LOADER_SECRET_KEY"], region_name="us-east-1", verify=CA)
results = []


def objects(prefix):
    keys, token = [], None
    while True:
        kw = {"Bucket": WAREHOUSE, "Prefix": prefix}
        if token:
            kw["ContinuationToken"] = token
        page = s3.list_objects_v2(**kw)
        keys += [o["Key"] for o in page.get("Contents", [])]
        if not page.get("IsTruncated"):
            return keys
        token = page["NextContinuationToken"]


def prefix_of(table):
    loc = table.metadata.location  # s3://<warehouse>/<prefix>
    return loc.split(f"{WAREHOUSE}/", 1)[1].rstrip("/") + "/"


def step(name, ok, detail):
    results.append(("OK" if ok else "FAIL", name, detail))


cat = catalog(WAREHOUSE)
if (NS,) not in cat.list_namespaces():
    cat.create_namespace(NS)
rows = pa.table({"id": pa.array(range(100), pa.int64()), "v": pa.array([f"x{i}" for i in range(100)])})

name = f"{NS}.t_{int(time.time())}"
t = cat.create_table(name, schema=rows.schema)
t.append(rows)
t.append(rows)
prefix = prefix_of(cat.load_table(name))
before = objects(prefix)
step("written", len(before) > 0, f"{len(before)} objects under the table's prefix")
try:
    cat.purge_table(name)
    step("purgeRequested=true", True, "taken")
except Exception as e:
    step("purgeRequested=true", "not supported" in str(e), f"refused: {str(e).splitlines()[-1][:90]}")
cat.drop_table(name)
gone = not cat.table_exists(name)
step("dropped from the catalog", gone, "not listed" if gone else "STILL LISTED")
time.sleep(2)
left = objects(prefix)
step("its files after the drop", True, f"{len(left)} of {len(before)} objects left: the drop does not remove them")
for k in left:
    s3.delete_object(Bucket=WAREHOUSE, Key=k)
after = objects(prefix)
step("its files deleted under its prefix", not after, f"{len(after)} left")

# The same, as the application does it: credentials from STS whose session
# policy lets them drop that one table and delete only its files.
import hashlib, json, requests  # noqa: E402
from urllib.parse import quote  # noqa: E402
from botocore.auth import S3SigV4Auth  # noqa: E402
from botocore.awsrequest import AWSRequest  # noqa: E402
from botocore.credentials import Credentials  # noqa: E402

def drop_policy(ns, table, prefix):
    """As quality::lake::drop_policy: the table's files (by its prefix and
    its ARN, as maintenance has them), and the warehouse's catalog to drop
    it: RustFS matches DeleteTable against the warehouse only."""
    bucket = f"arn:aws:s3:::{WAREHOUSE}"
    everything = [bucket, f"{bucket}/*"]
    return {"Version": "2012-10-17", "Statement": [
        {"Effect": "Allow", "Action": ["s3:GetBucketLocation", "s3:ListBucket"], "Resource": [bucket]},
        {"Effect": "Allow", "Action": ["s3:GetObject", "s3:DeleteObject"],
         "Resource": [f"{bucket}/{prefix}/*", f"{bucket}/.*/namespaces/{ns}/tables/{table}/*"]},
        {"Effect": "Allow", "Action": ["admin:GetTableCatalog", "admin:GetTableBucket", "admin:GetTableNamespace",
                                       "admin:GetTable", "admin:GetTableMetadata", "admin:GetTableMetadataLocation",
                                       "admin:CommitTable", "admin:SetTableMetadata", "admin:SetTableMetadataLocation",
                                       "admin:DeleteTable", "admin:DeleteTableNamespace"],
         "Resource": everything}]}

def sts_creds(policy):
    sts = boto3.client("sts", endpoint_url="https://localhost", aws_access_key_id=os.environ["RUSTFS_STS_ISSUER_ACCESS_KEY"],
                       aws_secret_access_key=os.environ["RUSTFS_STS_ISSUER_SECRET_KEY"], region_name="us-east-1", verify=CA)
    c = sts.assume_role(RoleArn="arn:aws:iam::000000000000:role/qd-job", RoleSessionName="drop-verify",
                        DurationSeconds=900, Policy=json.dumps(policy))["Credentials"]
    return Credentials(c["AccessKeyId"], c["SecretAccessKey"], c["SessionToken"])

def signed(creds, method, url):
    aws = AWSRequest(method=method, url=url, data=b"", headers={"x-amz-content-sha256": hashlib.sha256(b"").hexdigest()})
    S3SigV4Auth(creds, "s3", "us-east-1").add_auth(aws)
    return requests.request(method, url, headers=dict(aws.headers), verify=CA)

if (NS,) not in cat.list_namespaces():
    cat.create_namespace(NS)
stamp = int(time.time())
kept, target = f"t_kept_{stamp}", f"t_drop_{stamp}"
for n in (kept, target):
    t = cat.create_table(f"{NS}.{n}", schema=rows.schema)
    t.append(rows)
prefix = prefix_of(cat.load_table(f"{NS}.{target}")).rstrip("/")
kept_prefix = prefix_of(cat.load_table(f"{NS}.{kept}")).rstrip("/")
creds = sts_creds(drop_policy(NS, target, prefix))
base = f"https://localhost/iceberg/v1/{WAREHOUSE}/namespaces/{NS}/tables"
# Its files first, while the catalog still has it: RustFS refuses them after.
r = signed(creds, "GET", f"https://localhost/{WAREHOUSE}?list-type=2&prefix={quote(prefix + '/', safe='')}")
keys = [k.split("</Key>")[0] for k in r.text.split("<Key>")[1:]]
step("STS: its files listed", r.status_code == 200 and len(keys) > 0, f"HTTP {r.status_code}, {len(keys)} keys")
codes = {signed(creds, "DELETE", f"https://localhost/{WAREHOUSE}/{k}").status_code for k in keys}
left = objects(prefix + "/")
step("STS: its files deleted", codes <= {200, 204} and not left, f"HTTP {sorted(codes)}, {len(left)} left")
other = objects(kept_prefix + "/")[0]
r = signed(creds, "DELETE", f"https://localhost/{WAREHOUSE}/{other}")
step("STS: another table's file", r.status_code in (401, 403) and objects(kept_prefix + "/"), f"refused ({r.status_code})")
r = signed(creds, "DELETE", f"{base}/{target}")
step("STS: then its table dropped", r.status_code in (200, 204) and not cat.table_exists(f"{NS}.{target}"), f"HTTP {r.status_code}")
r = signed(creds, "DELETE", f"https://localhost/iceberg/v1/{WAREHOUSE}/namespaces/{NS}")
step("STS: its namespace while not empty", r.status_code == 409, f"HTTP {r.status_code}")
step("the other table untouched", cat.table_exists(f"{NS}.{kept}") and len(cat.load_table(f"{NS}.{kept}").scan().to_arrow()) == 100, "reads its 100 rows")

# Anything an earlier run left, removed the same way.
for ident in cat.list_tables(NS):
    old = cat.load_table(ident)
    keys = objects(prefix_of(old))
    cat.drop_table(ident)
    for k in keys:
        s3.delete_object(Bucket=WAREHOUSE, Key=k)
try:
    creds = sts_creds(drop_policy(NS, "none", "tables/none"))
    r = signed(creds, "DELETE", f"https://localhost/iceberg/v1/{WAREHOUSE}/namespaces/{NS}")
    step("STS: an empty namespace dropped", r.status_code in (200, 204) and (NS,) not in cat.list_namespaces(), f"HTTP {r.status_code}")
except Exception as e:
    step("an empty namespace dropped", False, f"{type(e).__name__}: {str(e)[:120]}")

for status, name, detail in results:
    print(f"[{status:4}] {name}: {detail}")
print("== all checks passed" if all(r[0] == "OK" for r in results) else "== some checks FAILED")
