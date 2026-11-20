"""The checks of tools/verify/rustfs_warehouse.sh.

Every call is signed with the main lake's platform admin keys. Prints
[OK  ] and [FAIL] for what is expected, [INFO] for what RustFS answers; no
key is printed. Exits 1 if a check fails.
"""

import hashlib
import json
import os

import boto3
import requests
from botocore.auth import S3SigV4Auth
from botocore.awsrequest import AWSRequest
from botocore.config import Config
from botocore.credentials import Credentials

D = os.environ["D"]
LAKE = f"http://{os.environ['LAKE_IP']}:9000"
W = "qd-tenant-0000000000000000000000000000000a"
V = "qd-tenant-0000000000000000000000000000000b"
FAILED = []


def keys():
    d = dict(line.strip().split("=", 1) for line in open(f"{D}/admin.env") if "=" in line)
    return d["ADMIN_ACCESS_KEY"], d["ADMIN_SECRET_KEY"]


ADMIN = keys()


def expect(name, wanted, got):
    print(f"[OK  ] {name}: {got}" if wanted == got else f"[FAIL] {name}: expected {wanted!r}, got {got!r}")
    if wanted != got:
        FAILED.append(name)


def info(name, what):
    print(f"[INFO] {name}: {what}")


def signed(method, path, body=None, headers=None):
    data = json.dumps(body).encode() if body is not None else b""
    h = {"x-amz-content-sha256": hashlib.sha256(data).hexdigest(), **(headers or {})}
    if body is not None:
        h["content-type"] = "application/json"
    aws = AWSRequest(method=method, url=LAKE + path, data=data, headers=h)
    S3SigV4Auth(Credentials(*ADMIN), "s3", "us-east-1").add_auth(aws)
    return requests.request(method, LAKE + path, data=data, headers=dict(aws.headers), timeout=30)


def short(r):
    return f"{r.status_code} {r.text[:200]}".strip()


s3 = boto3.client("s3", endpoint_url=LAKE, aws_access_key_id=ADMIN[0], aws_secret_access_key=ADMIN[1],
                  region_name="us-east-1",
                  config=Config(s3={"addressing_style": "path"}, retries={"max_attempts": 1}))

TABLE = {"name": "t1", "schema": {"type": "struct", "schema-id": 0,
                                  "fields": [{"id": 1, "name": "a", "required": False, "type": "string"}]}}


def catalog(warehouse, ns):
    made = signed("POST", f"/iceberg/v1/{warehouse}/namespaces", {"namespace": [ns], "properties": {}})
    table = signed("POST", f"/iceberg/v1/{warehouse}/namespaces/{ns}/tables", TABLE)
    return made, table


def namespaces(warehouse):
    r = signed("GET", f"/iceberg/v1/{warehouse}/namespaces")
    if r.status_code != 200:
        return short(r)
    return sorted(n[0] for n in r.json().get("namespaces", []))


def tables(warehouse, ns):
    r = signed("GET", f"/iceberg/v1/{warehouse}/namespaces/{ns}/tables")
    if r.status_code != 200:
        return short(r)
    return sorted(t["name"] for t in r.json().get("identifiers", []))


print("== (a) a missing warehouse")
expect("HEAD of a missing warehouse", 404, signed("HEAD", f"/{W}").status_code)
r = signed("PUT", f"/iceberg/v1/buckets/{W}")
info("the table bucket of a bucket that does not exist", short(r))
info("HEAD after it", signed("HEAD", f"/{W}").status_code)
info("its state after it", short(signed("GET", f"/iceberg/v1/buckets/{W}")))

print("== (b) the warehouse made: the bucket, then its table bucket")
head = signed("HEAD", f"/{W}").status_code
if head == 404:
    expect("the bucket made", 200, signed("PUT", f"/{W}").status_code)
else:
    info("the bucket was there already (made by the table bucket's call)", head)
expect("its table bucket", 200, signed("PUT", f"/iceberg/v1/buckets/{W}").status_code)
state = signed("GET", f"/iceberg/v1/buckets/{W}")
info("the table bucket read back", short(state))
expect("read back answers 200", 200, state.status_code)
made, table = catalog(W, "verify_ns")
expect("a namespace made through the catalog", 200, made.status_code)
expect("a table made in it", 200, table.status_code)
info("the table's answer", table.text[:120])
info("versioning of a new warehouse", s3.get_bucket_versioning(Bucket=W).get("Status"))

print("== (c) a second run")
expect("HEAD of the warehouse", 200, signed("HEAD", f"/{W}").status_code)
r = signed("PUT", f"/{W}")
info("the bucket made again", short(r))
expect("the table bucket again", 200, signed("PUT", f"/iceberg/v1/buckets/{W}").status_code)
expect("the namespace still there", ["verify_ns"], namespaces(W))
expect("the table still there", ["t1"], tables(W, "verify_ns"))
s3.put_object(Bucket=W, Key="verify_ns/data/x.txt", Body=b"synthetic")
expect("the table bucket a third time, with objects in it", 200,
       signed("PUT", f"/iceberg/v1/buckets/{W}").status_code)
expect("the table still there after it", ["t1"], tables(W, "verify_ns"))

print("== (d) with versioning on")
s3.put_bucket_versioning(Bucket=W, VersioningConfiguration={"Status": "Enabled"})
expect("versioning on the made warehouse", "Enabled", s3.get_bucket_versioning(Bucket=W).get("Status"))
made, table = catalog(W, "verify_ns2")
expect("a namespace made after versioning", 200, made.status_code)
expect("a table made after versioning", 200, table.status_code)
expect("the table bucket again, versioned", 200, signed("PUT", f"/iceberg/v1/buckets/{W}").status_code)
expect("both namespaces listed", ["verify_ns", "verify_ns2"], namespaces(W))
expect("bucket V made", 200, signed("PUT", f"/{V}").status_code)
s3.put_bucket_versioning(Bucket=V, VersioningConfiguration={"Status": "Enabled"})
expect("the table bucket of a bucket versioned first", 200,
       signed("PUT", f"/iceberg/v1/buckets/{V}").status_code)
made, table = catalog(V, "verify_ns")
expect("its namespace", 200, made.status_code)
expect("its table", 200, table.status_code)

print("== (e) outside qd-tenant-*")
expect("a bucket outside qd-tenant-* refused", 403, signed("PUT", "/qd-backups").status_code)
r = signed("PUT", "/iceberg/v1/buckets/qd-backups")
info("its table bucket", short(r))

print()
print("ALL OK" if not FAILED else f"SOME CHECKS FAILED: {len(FAILED)}")
raise SystemExit(1 if FAILED else 0)
