"""Step (d) of tools/verify/rustfs_source_proxy.sh.

With each lake's platform admin keys alone (never root), the replication
from the source, through the overlay's proxy on its loopback
(`lake-1:9100`), into the replica behind its gate (the `app` machine's
certificate presented there). Prints [OK  ]/[FAIL]; no key is printed.
"""

import hashlib
import json
import os
import socket
import sys
import time

import boto3
import requests
from botocore.auth import S3SigV4Auth
from botocore.awsrequest import AWSRequest
from botocore.config import Config
from botocore.credentials import Credentials
from requests.adapters import HTTPAdapter

D = os.environ["D"]
CA, APP = f"{D}/ca.crt", (f"{D}/app-1.crt", f"{D}/app-1.key")
NAMES = {"replica-1": os.environ["REPLICA_IP"], "lake-1": os.environ["SOURCE_IP"]}
_gai = socket.getaddrinfo
socket.getaddrinfo = lambda host, *a, **k: _gai(NAMES.get(host, host), *a, **k)
REPLICA, SOURCE = "https://replica-1:9000", "https://lake-1:9000"
B = "qd-tenant-verify"
FAILED = []


def keys(name):
    d = dict(line.strip().split("=", 1) for line in open(f"{D}/{name}") if "=" in line)
    return d["ADMIN_ACCESS_KEY"], d["ADMIN_SECRET_KEY"]


ADMIN_R, ADMIN_S = keys("admin-replica.env"), keys("admin-source.env")


def expect(name, wanted, got):
    print(f"[OK  ] {name}: {got}" if wanted == got else f"[FAIL] {name}: expected {wanted!r}, got {got!r}")
    if wanted != got:
        FAILED.append(name)


class SigV4(HTTPAdapter):
    def __init__(self, creds):
        super().__init__()
        self.creds = Credentials(*creds)

    def add_headers(self, request, **kw):
        body = request.body or b""
        body = body.encode() if isinstance(body, str) else body
        request.headers["x-amz-content-sha256"] = hashlib.sha256(body).hexdigest()
        request.headers["content-length"] = str(len(body))
        request.headers.pop("connection", None)
        aws = AWSRequest(method=request.method, url=request.url, data=body, headers=dict(request.headers))
        S3SigV4Auth(self.creds, "s3", "us-east-1").add_auth(aws)
        request.headers.update(dict(aws.headers))


def admin(base, creds, method, path, body=None):
    s = requests.Session()
    s.mount(base, SigV4(creds))
    return s.request(method, base + path, data=json.dumps(body) if body else b"", verify=CA, cert=APP, timeout=30)


def s3(base, creds):
    return boto3.client(
        "s3", endpoint_url=base, aws_access_key_id=creds[0], aws_secret_access_key=creds[1],
        region_name="us-east-1", verify=CA,
        config=Config(s3={"addressing_style": "path"}, retries={"max_attempts": 1},
                      client_cert=APP, connect_timeout=5, read_timeout=30))


def code(fn):
    try:
        fn()
        return 200
    except Exception as e:
        return getattr(e, "response", {}).get("ResponseMetadata", {}).get("HTTPStatusCode", type(e).__name__)


src, dst = s3(SOURCE, ADMIN_S), s3(REPLICA, ADMIN_R)


def arrives(key, within):
    for _ in range(within):
        try:
            return dst.head_object(Bucket=B, Key=key)
        except Exception:
            time.sleep(1)
    return None


def status(key):
    try:
        return src.head_object(Bucket=B, Key=key).get("ReplicationStatus")
    except Exception as e:
        return type(e).__name__


def restarted(step, within, label):
    """Step (f), once the shell restarted the source's RustFS. `left`: an
    object written as soon as the source answers again, while the proxy
    may still be in the old namespace. `after`: once the proxy is back, an
    object written then must reach the replica, locked, within WITHIN
    seconds; where the first one is, is noted."""
    start = time.time()
    if step == "summary":
        for o in src.list_objects_v2(Bucket=B).get("Contents", []):
            on = "on" if arrives(o["Key"], 1) else "not on"
            print(f"[NOTE] at the end, {o['Key']}: {on} the replica, its status {status(o['Key'])}")
    elif step == "status":
        on = "on" if arrives(f"{label}-left.txt", 1) else "not on"
        print(f"[NOTE] {label}: {label}-left.txt {on} the replica, its status {status(f'{label}-left.txt')}")
    elif step == "left":
        for _ in range(within):
            if code(lambda: src.put_object(Bucket=B, Key=f"{label}-left.txt", Body=b"synthetic")) == 200:
                print(f"[NOTE] {label}: its first object written {int(time.time() - start)} s after asking")
                break
            time.sleep(1)
        else:
            expect(f"{label}: the source takes an object after its restart", 200, "no answer")
    else:
        src.put_object(Bucket=B, Key=f"{label}-after.txt", Body=b"synthetic")
        h = arrives(f"{label}-after.txt", within)
        expect(f"{label}: an object written after the restart reaches the replica within {within} s, locked",
               "COMPLIANCE", h and h.get("ObjectLockMode"))
        print(f"[NOTE] {label}: the next {'on the replica within' if h else 'not on the replica after'} "
              f"{int(time.time() - start)} s, its status on the source {status(f'{label}-after.txt')}; the first "
              f"{'on' if arrives(f'{label}-left.txt', 5) else 'not on'} the replica, its status "
              f"{status(f'{label}-left.txt')}")
    raise SystemExit(1 if FAILED else 0)


if len(sys.argv) == 5 and sys.argv[1] == "restarted":
    restarted(sys.argv[2], int(sys.argv[3]), sys.argv[4].replace(" ", "-"))
expect("source: the admin keys read info", 200, admin(SOURCE, ADMIN_S, "GET", "/rustfs/admin/v3/info").status_code)
expect("replica: the bucket with object lock", 200,
       code(lambda: dst.create_bucket(Bucket=B, ObjectLockEnabledForBucket=True)))
lock = {"ObjectLockEnabled": "Enabled", "Rule": {"DefaultRetention": {"Mode": "COMPLIANCE", "Days": 1}}}
expect("replica: default retention", 200, code(lambda: dst.put_object_lock_configuration(
    Bucket=B, ObjectLockConfiguration=lock)))
expect("source: the bucket", 200, code(lambda: src.create_bucket(Bucket=B)))
expect("source: versioning", 200, code(lambda: src.put_bucket_versioning(
    Bucket=B, VersioningConfiguration={"Status": "Enabled"})))
target = {"credentials": {"accessKey": ADMIN_R[0], "secretKey": ADMIN_R[1]}, "targetbucket": B,
          "secure": True, "type": "replication", "region": "us-east-1", "endpoint": "lake-1:9100"}
r = admin(SOURCE, ADMIN_S, "PUT", f"/rustfs/admin/v3/set-remote-target?bucket={B}", target)
expect("the remote target through the overlay's proxy (lake-1:9100)", 200, r.status_code)
if r.status_code != 200:
    print(f"[INFO] its answer: {r.text[:200]}")
ARN = r.text.strip().strip('"')
rule = {"Role": ARN, "Rules": [{
    "ID": "to-the-replica", "Status": "Enabled", "Priority": 1, "Filter": {"Prefix": ""},
    "DeleteMarkerReplication": {"Status": "Disabled"}, "Destination": {"Bucket": ARN}}]}
expect("the rule", 200, code(lambda: src.put_bucket_replication(Bucket=B, ReplicationConfiguration=rule)))
src.put_object(Bucket=B, Key="synthetic.txt", Body=b"synthetic")
seen = None
for i in range(30):
    try:
        h = dst.head_object(Bucket=B, Key="synthetic.txt")
        seen = i
        break
    except Exception:
        time.sleep(1)
expect("an object written on the source reaches the replica", True, seen is not None)
if seen is not None:
    expect("locked there", "COMPLIANCE", h.get("ObjectLockMode"))
raise SystemExit(1 if FAILED else 0)
