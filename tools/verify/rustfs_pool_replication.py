"""The replication step of tools/verify/rustfs_pool_replication.sh.

With each lake's platform admin keys alone (never root): a bucket per way
out, its remote target set once through pool-1's admin API, then an object
written through every node of the pool must reach the replica, behind its
gate, locked. Prints [OK  ]/[FAIL]/[NOTE]; no key is printed.
"""

import hashlib
import json
import os
import socket
import subprocess
import sys
import time

import boto3
import requests
from botocore.auth import S3SigV4Auth
from botocore.awsrequest import AWSRequest
from botocore.config import Config
from botocore.credentials import Credentials
from requests.adapters import HTTPAdapter

D, ALIAS, WAIT = os.environ["D"], os.environ["ALIAS"], int(os.environ["WAIT"])
CA, APP = f"{D}/ca.crt", (f"{D}/app-1.crt", f"{D}/app-1.key")
NAMES = dict(p.split("=", 1) for p in os.environ["ADDRS"].strip(",").split(","))
_gai = socket.getaddrinfo
socket.getaddrinfo = lambda host, *a, **k: _gai(NAMES.get(host, host), *a, **k)
NODES = [f"pool-{i}" for i in range(1, 5)]
REPLICA = "https://replica-1:9000"
FAILED = []


def keys(name):
    d = dict(line.strip().split("=", 1) for line in open(f"{D}/{name}") if "=" in line)
    return d["ADMIN_ACCESS_KEY"], d["ADMIN_SECRET_KEY"]


ADMIN_P, ADMIN_R = keys("admin-pool.env"), keys("admin-replica.env")


def expect(name, wanted, got):
    print(f"[OK  ] {name}: {got}" if wanted == got else f"[FAIL] {name}: expected {wanted!r}, got {got!r}")
    if wanted != got:
        FAILED.append(name)


def note(text):
    print(f"[NOTE] {text}")


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
    return s.request(method, base + path, data=json.dumps(body) if body else b"", verify=CA, cert=APP, timeout=60)


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


node = {n: s3(f"https://{n}:9010", ADMIN_P) for n in NODES}
dst = s3(REPLICA, ADMIN_R)
POOL1 = "https://pool-1:9010"


def arrives(bucket, key, within):
    for _ in range(within):
        try:
            return dst.head_object(Bucket=bucket, Key=key)
        except Exception:
            time.sleep(1)
    return None


def status(bucket, key, via):
    try:
        return node[via].head_object(Bucket=bucket, Key=key).get("ReplicationStatus")
    except Exception as e:
        return type(e).__name__


def target(bucket, endpoint, secure):
    t = {"credentials": {"accessKey": ADMIN_R[0], "secretKey": ADMIN_R[1]}, "targetbucket": bucket,
         "secure": secure, "type": "replication", "region": "us-east-1", "endpoint": endpoint}
    return admin(POOL1, ADMIN_P, "PUT", f"/rustfs/admin/v3/set-remote-target?bucket={bucket}", t)


def buckets(bucket):
    lock = {"ObjectLockEnabled": "Enabled", "Rule": {"DefaultRetention": {"Mode": "COMPLIANCE", "Days": 1}}}
    r = code(lambda: dst.create_bucket(Bucket=bucket, ObjectLockEnabledForBucket=True))
    r = f"{r}/{code(lambda: dst.put_object_lock_configuration(Bucket=bucket, ObjectLockConfiguration=lock))}"
    r = f"{r}/{code(lambda: node['pool-1'].create_bucket(Bucket=bucket))}"
    v = {"Status": "Enabled"}
    return f"{r}/{code(lambda: node['pool-2'].put_bucket_versioning(Bucket=bucket, VersioningConfiguration=v))}"


def way(label, bucket, endpoints, secure):
    """The first of ENDPOINTS RustFS takes as the bucket's target; the
    refusals before it are notes, all of them refused a failure."""
    print(f"-- ({label}) endpoints {endpoints}, secure={secure}")
    expect(f"({label}) buckets on both lakes", "200/200/200/200", buckets(bucket))
    for endpoint in endpoints:
        r = target(bucket, endpoint, secure)
        if r.status_code == 200:
            break
        note(f"({label}) {endpoint} refused as a target: {r.status_code} {r.text[150:600]}")
    expect(f"({label}) the remote target set through pool-1 ({endpoint})", 200, r.status_code)
    if r.status_code != 200:
        return False
    arn = r.text.strip().strip('"')
    rule = {"Role": arn, "Rules": [{
        "ID": "qd-replica", "Status": "Enabled", "Priority": 1, "Filter": {"Prefix": ""},
        "DeleteMarkerReplication": {"Status": "Disabled"}, "Destination": {"Bucket": arn}}]}
    expect(f"({label}) the rule", 200,
           code(lambda: node["pool-3"].put_bucket_replication(Bucket=bucket, ReplicationConfiguration=rule)))
    for n in NODES:
        node[n].put_object(Bucket=bucket, Key=f"{n}.txt", Body=b"synthetic")
    for n in NODES:
        h = arrives(bucket, f"{n}.txt", WAIT)
        expect(f"({label}) written through {n}: on the replica, locked", "COMPLIANCE", h and h.get("ObjectLockMode"))
        expect(f"({label}) … its status, asked of another node", "COMPLETED",
               status(bucket, f"{n}.txt", NODES[(NODES.index(n) + 1) % 4]))
    return True


def proxies(n, state):
    subprocess.run([os.environ["PROXIES"], n, state], check=True)


ways = {"a": ("qd-tenant-vianame", [f"{ALIAS}:9100"], True),
        # RustFS refuses a loopback address or `localhost` as a target.
        "b": ("qd-tenant-plain", ["127.0.0.1:9101", "localhost:9101", f"{ALIAS}:9101"], False)}


def after_restart(label, mode):
    """`restarted LABEL check|note`, once the shell restarted pool-1's
    RustFS: an object written through pool-1 into each way's bucket. With
    `check` it must reach the replica, locked, within WAIT seconds; with
    `note` whether and when it does is noted (another node may replicate
    it)."""
    for bucket, _, _ in ways.values():
        node["pool-1"].put_object(Bucket=bucket, Key=f"{label}.txt", Body=b"synthetic")
    start, seen = time.time(), {}
    while len(seen) < len(ways) and time.time() - start < WAIT:
        for k, (bucket, _, _) in ways.items():
            if k not in seen and arrives(bucket, f"{label}.txt", 1):
                seen[k] = int(time.time() - start)
    for k, (bucket, _, _) in ways.items():
        if mode == "check":
            h = arrives(bucket, f"{label}.txt", 1)
            expect(f"({k}) {label}: written through pool-1 after the restart, on the replica, locked",
                   "COMPLIANCE", h and h.get("ObjectLockMode"))
        when = f"on the replica within {seen[k]} s" if k in seen else f"not on the replica after {WAIT} s"
        note(f"({k}) {label}: {when}; its status asked of pool-3: {status(bucket, f'{label}.txt', 'pool-3')}")
        if mode == "check":
            note(f"({k}) … and the control's object: {'on' if arrives(bucket, 'control.txt', 5) else 'not on'} "
                 f"the replica, its status {status(bucket, 'control.txt', 'pool-3')}")
    raise SystemExit(1 if FAILED else 0)


def race(count):
    """`race N`: N objects, each written through the next node into each
    way's bucket; once its status, asked of another node, says COMPLETED,
    one look at the replica at once (no retry), then looks every 0.2 s for
    10 s if that one did not find it. Counts which, and what the first look
    met; also the order the restart's note used (a look first, then the
    status)."""
    tally = {}
    for i in range(count):
        via, asked = NODES[i % 4], NODES[(i + 2) % 4]
        for k, (bucket, _, _) in ways.items():
            key = f"race-{i}.txt"
            node[via].put_object(Bucket=bucket, Key=key, Body=b"synthetic")
            t0, s = time.time(), None
            while time.time() - t0 < 60:
                s = status(bucket, key, asked)
                if s in ("COMPLETED", "FAILED"):
                    break
                time.sleep(0.1)
            done = time.time() - t0
            try:
                dst.head_object(Bucket=bucket, Key=key)
                first = "found"
            except Exception as e:
                first = getattr(e, "response", {}).get("Error", {}).get("Code") or type(e).__name__
            later = None
            if first != "found":
                t1 = time.time()
                while time.time() - t1 < 10 and later is None:
                    try:
                        dst.head_object(Bucket=bucket, Key=key)
                        later = round(time.time() - t1, 1)
                    except Exception:
                        time.sleep(0.2)
            r = f"({k}) status {s}, first look {first}" + (f", found {later} s later" if first != "found" else "")
            tally[r] = tally.get(r, 0) + 1
            if first != "found":
                note(f"({k}) race-{i} via {via}, asked {asked}: {s} after {done:.1f} s; first look {first}; later {later}")
    for r, n in sorted(tally.items()):
        note(f"race: {n} x {r}")
    # The note's order: a look of up to 5 s, then the status.
    look_first = {}
    for i in range(count):
        for k, (bucket, _, _) in ways.items():
            key = f"look-{i}.txt"
            node[NODES[i % 4]].put_object(Bucket=bucket, Key=key, Body=b"synthetic")
            h = arrives(bucket, key, 5)
            r = f"({k}) look {'found' if h else 'not found'}, then status {status(bucket, key, 'pool-3')}"
            look_first[r] = look_first.get(r, 0) + 1
    for r, n in sorted(look_first.items()):
        note(f"look first: {n} x {r}")
    raise SystemExit(0)


if len(sys.argv) == 4 and sys.argv[1] == "restarted":
    after_restart(sys.argv[2], sys.argv[3])
if len(sys.argv) == 3 and sys.argv[1] == "race":
    race(int(sys.argv[2]))
working = {k: way(k, *v) for k, v in ways.items()}
print("-- (a) control: ALIAS served with a certificate that does not carry it")
expect("(control) buckets", "200/200/200/200", buckets("qd-tenant-noname"))
r = target("qd-tenant-noname", f"{ALIAS}:9102", True)
expect("(control) the remote target is refused", True, r.status_code != 200)
note(f"(control) its answer: {r.status_code} {r.text[150:600]}")

print("-- pool-1's proxies stopped")
proxies("pool-1", "off")
time.sleep(2)
live = {k: v for k, v in ways.items() if working[k]}
for k, (bucket, _, _) in live.items():
    node["pool-2"].put_object(Bucket=bucket, Key="pool-2-alone.txt", Body=b"synthetic")
    node["pool-1"].put_object(Bucket=bucket, Key="pool-1-alone.txt", Body=b"synthetic")
for k, (bucket, _, _) in live.items():
    h = arrives(bucket, "pool-2-alone.txt", WAIT)
    expect(f"({k}) written through pool-2 reaches the replica (pool-2's own proxy)", "COMPLIANCE",
           h and h.get("ObjectLockMode"))
    s = None
    for _ in range(30):
        s = status(bucket, "pool-1-alone.txt", "pool-3")
        if s in ("FAILED", "COMPLETED"):
            break
        time.sleep(1)
    # Which node replicates an object is RustFS's to say: seen both ways.
    note(f"({k}) written through pool-1 while its proxies are stopped: its status {s}")
proxies("pool-1", "on")
start = time.time()
for k, (bucket, _, _) in live.items():
    h = arrives(bucket, "pool-1-alone.txt", 300)
    expect(f"({k}) pool-1's proxies back: what it wrote meanwhile is on the replica, locked", "COMPLIANCE",
           h and h.get("ObjectLockMode"))
    note(f"({k}) … {int(time.time() - start)}s after they were started")
raise SystemExit(1 if FAILED else 0)
