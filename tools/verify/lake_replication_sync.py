"""The checks of tools/verify/lake_replication_sync.sh, one moment per MODE
(configured, replica_down, replica_back).

Every call is signed with a lake's platform admin keys. Prints [OK  ] and
[FAIL] for what is expected, [INFO] for what RustFS answers; no key is
printed. Exits 1 if a check fails.
"""

import hashlib
import json
import os
import sys
import time

import boto3
import requests
from botocore.auth import S3SigV4Auth
from botocore.awsrequest import AWSRequest
from botocore.config import Config
from botocore.credentials import Credentials

D = os.environ["D"]
MODE = os.environ["MODE"]
SOURCE, REPLICA = f"http://{os.environ['SOURCE_IP']}:9000", f"http://{os.environ['REPLICA_IP']}:9000"
B = "qd-tenant-0000000000000000000000000000000a"
FAILED = []


def keys(name):
    d = dict(line.strip().split("=", 1) for line in open(f"{D}/admin-{name}.env") if "=" in line)
    return d["ADMIN_ACCESS_KEY"], d["ADMIN_SECRET_KEY"]


ADMIN_S, ADMIN_R = keys("source"), keys("replica")


def expect(name, wanted, got):
    print(f"[OK  ] {name}: {got}" if wanted == got else f"[FAIL] {name}: expected {wanted!r}, got {got!r}")
    if wanted != got:
        FAILED.append(name)


def info(name, what):
    print(f"[INFO] {name}: {what}")


def admin(base, creds, method, path, body=None):
    data = json.dumps(body).encode() if body is not None else b""
    headers = {"x-amz-content-sha256": hashlib.sha256(data).hexdigest()}
    aws = AWSRequest(method=method, url=base + path, data=data, headers=headers)
    S3SigV4Auth(Credentials(*creds), "s3", "us-east-1").add_auth(aws)
    return requests.request(method, base + path, data=data, headers=dict(aws.headers), timeout=30)


def s3(base, creds):
    return boto3.client("s3", endpoint_url=base, aws_access_key_id=creds[0], aws_secret_access_key=creds[1],
                        region_name="us-east-1",
                        config=Config(s3={"addressing_style": "path"}, retries={"max_attempts": 1},
                                      connect_timeout=3, read_timeout=10))


def metrics(label):
    """The bucket's replication metrics, whole, and its keys at each level."""
    r = admin(SOURCE, ADMIN_S, "GET", f"/rustfs/admin/v3/replicationmetrics?bucket={B}")
    info(f"replicationmetrics {label} (HTTP {r.status_code})", r.text[:2000])
    try:
        body = r.json()
    except ValueError:
        return {}
    shape = {k: (sorted(v.keys()) if isinstance(v, dict) else type(v).__name__) for k, v in body.items()}
    info(f"its fields {label}", json.dumps(shape))
    return body


def statuses(prefix):
    src = s3(SOURCE, ADMIN_S)
    out = {}
    for o in src.list_objects_v2(Bucket=B, Prefix=prefix).get("Contents", []):
        out[o["Key"]] = src.head_object(Bucket=B, Key=o["Key"]).get("ReplicationStatus")
    return out


def wait_statuses(prefix, wanted, seconds):
    got = {}
    for _ in range(seconds):
        got = statuses(prefix)
        if got and all(v == wanted for v in got.values()):
            return got
        time.sleep(1)
    return got


def configured():
    src, dst = s3(SOURCE, ADMIN_S), s3(REPLICA, ADMIN_R)
    dst.create_bucket(Bucket=B, ObjectLockEnabledForBucket=True)
    lock = {"ObjectLockEnabled": "Enabled", "Rule": {"DefaultRetention": {"Mode": "COMPLIANCE", "Days": 30}}}
    dst.put_object_lock_configuration(Bucket=B, ObjectLockConfiguration=lock)
    src.create_bucket(Bucket=B)
    src.put_bucket_versioning(Bucket=B, VersioningConfiguration={"Status": "Enabled"})
    metrics("before a rule")
    target = {"credentials": {"accessKey": ADMIN_R[0], "secretKey": ADMIN_R[1]}, "targetbucket": B,
              "secure": False, "type": "replication", "region": "us-east-1", "endpoint": "replica:9000"}
    r = admin(SOURCE, ADMIN_S, "PUT", f"/rustfs/admin/v3/set-remote-target?bucket={B}", target)
    expect("remote target", 200, r.status_code)
    arn = r.text.strip().strip('"')
    rule = {"Role": arn, "Rules": [{"ID": "qd-replica", "Status": "Enabled", "Priority": 1,
                                    "Filter": {"Prefix": ""},
                                    "DeleteMarkerReplication": {"Status": "Disabled"},
                                    "Destination": {"Bucket": arn}}]}
    src.put_bucket_replication(Bucket=B, ReplicationConfiguration=rule)
    metrics("configured, nothing written")
    for i in range(3):
        src.put_object(Bucket=B, Key=f"a/{i}.txt", Body=f"synthetic {i}".encode())
    got = wait_statuses("a/", "COMPLETED", 30)
    expect("three objects copied", {f"a/{i}.txt": "COMPLETED" for i in range(3)}, got)
    metrics("three objects copied")


def replica_down():
    src = s3(SOURCE, ADMIN_S)
    for i in range(3):
        src.put_object(Bucket=B, Key=f"b/{i}.txt", Body=f"synthetic down {i}".encode())
    info("statuses just after", json.dumps(statuses("b/")))
    metrics("the replica down, just after three writes")
    time.sleep(15)
    info("statuses after 15 s", json.dumps(statuses("b/")))
    metrics("the replica down, after 15 s")


def replica_back():
    got = wait_statuses("b/", "COMPLETED", 120)
    info("the objects written while it was down, after it came back", json.dumps(got))
    expect("they arrive by themselves", True, bool(got) and all(v == "COMPLETED" for v in got.values()))
    metrics("the replica back, everything copied")
    time.sleep(60)
    metrics("a minute later")


{"configured": configured, "replica_down": replica_down, "replica_back": replica_back}[MODE]()
sys.exit(1 if FAILED else 0)
