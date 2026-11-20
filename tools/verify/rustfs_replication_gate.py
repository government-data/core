"""The application's side of tools/verify/rustfs_replication_gate.sh.

Plays the application: every call is signed with a lake's platform admin
keys (qd-platform-admin, never root) and, at the replica's gate, presents
the `app` machine's certificate. Prints [OK  ]/[FAIL] for what is expected
and [INFO] for what RustFS answers that the decision records; no key or
secret is printed. Exits 1 if a check fails.
"""

import hashlib
import json
import os
import socket
import subprocess
import time

import boto3
import requests
from botocore.auth import S3SigV4Auth
from botocore.awsrequest import AWSRequest
from botocore.config import Config
from botocore.credentials import Credentials
from requests.adapters import HTTPAdapter

D, P = os.environ["D"], os.environ["P"]
CA = f"{D}/ca.crt"
APP = (f"{D}/app-1.crt", f"{D}/app-1.key")
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
    if wanted == got:
        print(f"[OK  ] {name}: {got}")
    else:
        print(f"[FAIL] {name}: expected {wanted!r}, got {got!r}")
        FAILED.append(name)


def info(name, what):
    print(f"[INFO] {name}: {what}")


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
    """A signed call to the admin API, presenting the app's certificate."""
    s = requests.Session()
    s.mount(base, SigV4(creds))
    data = json.dumps(body) if isinstance(body, (dict, list)) else (body or b"")
    return s.request(method, base + path, data=data, verify=CA, cert=APP, timeout=30)


def s3(base, creds):
    return boto3.client(
        "s3", endpoint_url=base, aws_access_key_id=creds[0], aws_secret_access_key=creds[1],
        region_name="us-east-1", verify=CA,
        config=Config(s3={"addressing_style": "path"}, retries={"max_attempts": 1},
                      client_cert=APP, connect_timeout=5, read_timeout=30))


def docker(*args):
    subprocess.run(["docker", *args], check=True, capture_output=True)


def code(fn):
    try:
        fn()
        return 200
    except Exception as e:  # botocore's ClientError carries the status
        return getattr(e, "response", {}).get("ResponseMetadata", {}).get("HTTPStatusCode", type(e).__name__)


def arrives(key, within):
    """Seconds until KEY is on the replica, or None."""
    start = time.time()
    while time.time() - start < within:
        try:
            dst.head_object(Bucket=B, Key=key)
            return round(time.time() - start)
        except Exception:
            time.sleep(1)
    return None


def status(key):
    return src.head_object(Bucket=B, Key=key).get("ReplicationStatus")


def metrics(label):
    r = admin(SOURCE, ADMIN_S, "GET", f"/rustfs/admin/v3/replicationmetrics?bucket={B}")
    info(f"replicationmetrics {label} ({r.status_code})", r.text[:900])
    return r


def targets(label):
    r = admin(SOURCE, ADMIN_S, "GET", f"/rustfs/admin/v3/list-remote-targets?bucket={B}&type=replication")
    try:
        body = r.json()
        show = [{k: t.get(k) for k in ("arn", "endpoint", "secure", "online", "totalDowntime", "lastOnline",
                                         "latency", "healthCheckDuration", "edge") if k in t}
                for t in (body or [])]
    except ValueError:
        show = r.text[:300]
    info(f"list-remote-targets {label} ({r.status_code})", json.dumps(show))
    return r


src, dst = s3(SOURCE, ADMIN_S), s3(REPLICA, ADMIN_R)

print("== (a) the replica's gate")
try:
    requests.get(REPLICA, verify=CA, timeout=10)
    expect("no client certificate", "refused at the handshake", "answered")
except requests.exceptions.SSLError:
    expect("no client certificate", "refused at the handshake", "refused at the handshake")
r = admin(REPLICA, ADMIN_R, "GET", "/rustfs/admin/v3/info")
expect("the admin keys read info through the gate, the app's certificate presented", 200, r.status_code)
body = (r.json() if r.status_code == 200 else {}).get("info") or {}
info("info.info's fields", sorted(body))
backend = body.get("backend") or {}
info("info.info.backend", json.dumps(backend))
expect("one drive online, none offline", (1, 0), (backend.get("onlineDisks"), backend.get("offlineDisks")))
bad = admin(REPLICA, (ADMIN_R[0], "0" * 64), "GET", "/rustfs/admin/v3/info")
expect("a wrong secret", 403, bad.status_code)
info("its code", bad.text.split("<Code>")[1].split("<")[0] if "<Code>" in bad.text else bad.text[:80])
bad = admin(REPLICA, ("qd-nobody", "0" * 64), "GET", "/rustfs/admin/v3/info")
expect("an unknown access key", 403, bad.status_code)
info("its code", bad.text.split("<Code>")[1].split("<")[0] if "<Code>" in bad.text else bad.text[:80])

print("== (b) the buckets")
expect("replica: the bucket made with object lock", 200,
       code(lambda: dst.create_bucket(Bucket=B, ObjectLockEnabledForBucket=True)))
expect("replica: versioning with it", "Enabled", dst.get_bucket_versioning(Bucket=B).get("Status"))
lock = {"ObjectLockEnabled": "Enabled", "Rule": {"DefaultRetention": {"Mode": "COMPLIANCE", "Days": 1}}}
expect("replica: default retention COMPLIANCE, 1 day", 200,
       code(lambda: dst.put_object_lock_configuration(Bucket=B, ObjectLockConfiguration=lock)))
expect("source: the bucket", 200, code(lambda: src.create_bucket(Bucket=B)))
expect("source: its table bucket (Iceberg), as a tenant's warehouse", 200,
       admin(SOURCE, ADMIN_S, "PUT", f"/iceberg/v1/buckets/{B}").status_code)
expect("source: versioning", 200,
       code(lambda: src.put_bucket_versioning(Bucket=B, VersioningConfiguration={"Status": "Enabled"})))
src.put_object(Bucket=B, Key="before.txt", Body=b"synthetic before")

print("== (c) the remote target")
target = {"credentials": {"accessKey": ADMIN_R[0], "secretKey": ADMIN_R[1]}, "targetbucket": B,
          "secure": True, "type": "replication", "region": "us-east-1"}
r = admin(SOURCE, ADMIN_S, "PUT", f"/rustfs/admin/v3/set-remote-target?bucket={B}",
          {**target, "endpoint": "replica-1:9000"})
expect("straight at the replica's gate", "refused", "refused" if r.status_code >= 300 else "set")
info("its answer", f"{r.status_code} {r.text[:200]}")
r = admin(SOURCE, ADMIN_S, "PUT", f"/rustfs/admin/v3/set-remote-target?bucket={B}",
          {**target, "endpoint": "lake-1:9101"})
expect("through the worker's proxy as it is (lake-1:9101)", "refused, HTTP/2 frame error",
       "refused, HTTP/2 frame error" if r.status_code >= 300 and "FRAME_SIZE_ERROR" in r.text else
       f"{r.status_code} {r.text[:120]}")
r = admin(SOURCE, ADMIN_S, "PUT", f"/rustfs/admin/v3/set-remote-target?bucket={B}",
          {**target, "endpoint": "lake-1:9100"})
expect("through the proxy offering HTTP/1.1 alone (lake-1:9100)", 200, r.status_code)
ARN = r.text.strip().strip('"')
info("the ARN", ARN)
targets("after it")

print("== (d) the rule, and objects")
rule = {"Role": ARN, "Rules": [{
    "ID": "to-the-replica", "Status": "Enabled", "Priority": 1, "Filter": {"Prefix": ""},
    "DeleteMarkerReplication": {"Status": "Disabled"},
    "ExistingObjectReplication": {"Status": "Enabled"},
    "Destination": {"Bucket": ARN}}]}
expect("replication rule, no delete markers, existing objects", 200,
       code(lambda: src.put_bucket_replication(Bucket=B, ReplicationConfiguration=rule)))
got = src.get_bucket_replication(Bucket=B)["ReplicationConfiguration"]["Rules"][0]
info("the rule read back", json.dumps({k: got.get(k) for k in ("ID", "Status", "DeleteMarkerReplication",
                                                                  "DeleteReplication", "ExistingObjectReplication")}))
src.put_object(Bucket=B, Key="after.txt", Body=b"synthetic after")
t = arrives("after.txt", 30)
expect("an object written after the rule reaches the replica", True, t is not None)
info("in seconds", t)
expect("the source says COMPLETED", "COMPLETED", status("after.txt"))
h = dst.head_object(Bucket=B, Key="after.txt")
expect("the replica says REPLICA", "REPLICA", h.get("ReplicationStatus"))
expect("the replica's copy is locked COMPLIANCE", "COMPLIANCE", h.get("ObjectLockMode"))
info("retained until", h.get("ObjectLockRetainUntilDate"))
vid = h.get("VersionId")
expect("its version cannot be deleted on the replica, even by the admin", "refused",
       "refused" if code(lambda: dst.delete_object(Bucket=B, Key="after.txt", VersionId=vid)) != 200 else "deleted")
t = arrives("before.txt", 60)
info("an object written before the rule (ExistingObjectReplication)", f"arrived in {t} s" if t is not None
     else "not on the replica within 60 s")
info("its source status", status("before.txt"))
metrics("after two objects")
B2 = "qd-tenant-verify-existing"
for c in (src, dst):
    c.create_bucket(Bucket=B2, ObjectLockEnabledForBucket=c is dst)
src.put_bucket_versioning(Bucket=B2, VersioningConfiguration={"Status": "Enabled"})
src.put_object(Bucket=B2, Key="before.txt", Body=b"synthetic before")
arn2 = admin(SOURCE, ADMIN_S, "PUT", f"/rustfs/admin/v3/set-remote-target?bucket={B2}",
             {**target, "targetbucket": B2, "endpoint": "lake-1:9100"}).text.strip().strip('"')
plain = {"Role": arn2, "Rules": [{**rule["Rules"][0], "Destination": {"Bucket": arn2}}]}
del plain["Rules"][0]["ExistingObjectReplication"]
src.put_bucket_replication(Bucket=B2, ReplicationConfiguration=plain)
seen = None
for i in range(60):
    try:
        dst.head_object(Bucket=B2, Key="before.txt")
        seen = i
        break
    except Exception:
        time.sleep(1)
info("an object written before a rule without ExistingObjectReplication",
     f"arrived in {seen} s" if seen is not None else "not on the replica within 60 s")

print("== (e) a second configuration run")
info("replica: the bucket again", code(lambda: dst.create_bucket(Bucket=B, ObjectLockEnabledForBucket=True)))
expect("replica: the retention again", 200,
       code(lambda: dst.put_object_lock_configuration(Bucket=B, ObjectLockConfiguration=lock)))
info("source: the table bucket again",
     admin(SOURCE, ADMIN_S, "PUT", f"/iceberg/v1/buckets/{B}").status_code)
r = admin(SOURCE, ADMIN_S, "PUT", f"/rustfs/admin/v3/set-remote-target?bucket={B}",
          {**target, "endpoint": "lake-1:9100"})
info("source: the remote target again", f"{r.status_code} {r.text.strip()[:200]}")
same = r.status_code == 200 and r.text.strip().strip('"') == ARN
info("… the same ARN", same)
rt = targets("after a second run")
try:
    count = len(rt.json() or [])
except ValueError:
    count = None
expect("one remote target after two runs", 1, count)
expect("source: the rule again", 200,
       code(lambda: src.put_bucket_replication(Bucket=B, ReplicationConfiguration=rule)))
src.put_object(Bucket=B, Key="second.txt", Body=b"synthetic second")
expect("an object after the second run arrives", True, arrives("second.txt", 30) is not None)

print("== (f) a delete on the source")
src.delete_object(Bucket=B, Key="after.txt")
time.sleep(5)
expect("the replica still serves it", 200, code(lambda: dst.head_object(Bucket=B, Key="after.txt")))
vs = dst.list_object_versions(Bucket=B, Prefix="after.txt")
expect("no delete marker on the replica", 0, len(vs.get("DeleteMarkers", [])))

print("== (g) the replica's gate down, then back")
docker("stop", f"{P}-replica-gate")
src.put_object(Bucket=B, Key="while-down.txt", Body=b"synthetic down")
time.sleep(10)
info("the source's status while down, after 10 s", status("while-down.txt"))
metrics("while down")
targets("while down")
docker("start", f"{P}-replica-gate")
NAMES["replica-1"] = subprocess.run(
    ["docker", "inspect", "-f", f'{{{{(index .NetworkSettings.Networks "{P}").IPAddress}}}}', f"{P}-replica-gate"],
    capture_output=True, text=True, check=True).stdout.strip()
t = arrives("while-down.txt", 300)
info("after the gate is back, it arrives in", f"{t} s" if t is not None else "not within 300 s")
info("the source's status then", status("while-down.txt"))
src.put_object(Bucket=B, Key="after-back.txt", Body=b"synthetic back")
expect("a new object after the gate is back arrives", True, arrives("after-back.txt", 60) is not None)
metrics("after it is back")
targets("after it is back")
r = admin(SOURCE, ADMIN_S, "GET", f"/rustfs/admin/v3/replication/diff?bucket={B}")
info(f"replication/diff ({r.status_code})", r.text[:400])

print()
print("ALL OK" if not FAILED else f"SOME CHECKS FAILED: {len(FAILED)}")
raise SystemExit(1 if FAILED else 0)
