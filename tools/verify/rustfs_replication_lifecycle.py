"""The checks of tools/verify/rustfs_replication_lifecycle.sh.

Every call is signed with a lake's platform admin keys. Prints [OK  ] and
[FAIL] for what is expected, [INFO] for what RustFS answers; no key is
printed. Exits 1 if a check fails.
"""

import hashlib
import json
import os
import time

import boto3
import requests
from botocore.auth import S3SigV4Auth
from botocore.awsrequest import AWSRequest
from botocore.config import Config
from botocore.credentials import Credentials

D = os.environ["D"]
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
                        config=Config(s3={"addressing_style": "path"}, retries={"max_attempts": 1}))


def code(fn):
    try:
        fn()
        return 200
    except Exception as e:
        meta = getattr(e, "response", {})
        return (meta.get("ResponseMetadata", {}).get("HTTPStatusCode", type(e).__name__),
                meta.get("Error", {}).get("Code"))


src, dst = s3(SOURCE, ADMIN_S), s3(REPLICA, ADMIN_R)

print("== (a) the replica's bucket, COMPLIANCE 30 days")
expect("made with object lock", 200, code(lambda: dst.create_bucket(Bucket=B, ObjectLockEnabledForBucket=True)))
lock = {"ObjectLockEnabled": "Enabled", "Rule": {"DefaultRetention": {"Mode": "COMPLIANCE", "Days": 30}}}
expect("default retention set", 200, code(lambda: dst.put_object_lock_configuration(
    Bucket=B, ObjectLockConfiguration=lock)))
got = dst.get_object_lock_configuration(Bucket=B)["ObjectLockConfiguration"]
expect("read back", lock, got)
info("made again", code(lambda: dst.create_bucket(Bucket=B, ObjectLockEnabledForBucket=True)))
expect("set again", 200, code(lambda: dst.put_object_lock_configuration(Bucket=B, ObjectLockConfiguration=lock)))
expect("versioning came with it", "Enabled", dst.get_bucket_versioning(Bucket=B).get("Status"))
plain = "qd-tenant-0000000000000000000000000000000b"
dst.create_bucket(Bucket=plain)
info("a bucket made without object lock, given a default retention",
     code(lambda: dst.put_object_lock_configuration(Bucket=plain, ObjectLockConfiguration=lock)))

print("== (b) the source: versioning, lifecycle, replication")
expect("bucket", 200, code(lambda: src.create_bucket(Bucket=B)))
expect("table bucket", 200, admin(SOURCE, ADMIN_S, "PUT", f"/iceberg/v1/buckets/{B}").status_code)
expect("versioning", 200, code(lambda: src.put_bucket_versioning(
    Bucket=B, VersioningConfiguration={"Status": "Enabled"})))
life = {"Rules": [{"ID": "qd-noncurrent-7d", "Status": "Enabled", "Filter": {"Prefix": ""},
                   "NoncurrentVersionExpiration": {"NoncurrentDays": 7}}]}
expect("lifecycle: noncurrent versions expire after 7 days", 200, code(
    lambda: src.put_bucket_lifecycle_configuration(Bucket=B, LifecycleConfiguration=life)))
got = src.get_bucket_lifecycle_configuration(Bucket=B)["Rules"]
info("read back", json.dumps(got))
expect("its rule read back", (7, "Enabled"), (got[0].get("NoncurrentVersionExpiration", {}).get("NoncurrentDays"),
                                             got[0].get("Status")))
target = {"credentials": {"accessKey": ADMIN_R[0], "secretKey": ADMIN_R[1]}, "targetbucket": B,
          "secure": False, "type": "replication", "region": "us-east-1", "endpoint": "replica:9000"}
r = admin(SOURCE, ADMIN_S, "PUT", f"/rustfs/admin/v3/set-remote-target?bucket={B}", target)
expect("remote target", 200, r.status_code)
ARN = r.text.strip().strip('"')
rule = {"Role": ARN, "Rules": [{"ID": "qd-replica", "Status": "Enabled", "Priority": 1, "Filter": {"Prefix": ""},
                                "DeleteMarkerReplication": {"Status": "Disabled"},
                                "Destination": {"Bucket": ARN}}]}
expect("replication rule", 200, code(lambda: src.put_bucket_replication(Bucket=B, ReplicationConfiguration=rule)))
expect("lifecycle again, with replication on", 200, code(
    lambda: src.put_bucket_lifecycle_configuration(Bucket=B, LifecycleConfiguration=life)))
expect("the lifecycle kept", 1, len(src.get_bucket_lifecycle_configuration(Bucket=B)["Rules"]))
expect("the replication rule kept", "qd-replica",
       src.get_bucket_replication(Bucket=B)["ReplicationConfiguration"]["Rules"][0]["ID"])
src.put_object(Bucket=B, Key="a.txt", Body=b"synthetic one")
src.put_object(Bucket=B, Key="a.txt", Body=b"synthetic two")
seen = None
for i in range(30):
    vs = dst.list_object_versions(Bucket=B, Prefix="a.txt").get("Versions", [])
    if len(vs) == 2:
        seen = i
        break
    time.sleep(1)
expect("both versions reach the replica with the lifecycle on", True, seen is not None)
h = dst.head_object(Bucket=B, Key="a.txt")
expect("locked COMPLIANCE there", "COMPLIANCE", h.get("ObjectLockMode"))
info("retained until (30 days)", h.get("ObjectLockRetainUntilDate"))
vs = src.list_object_versions(Bucket=B, Prefix="a.txt").get("Versions", [])
expect("the source keeps one noncurrent version (not expired before 7 days)", 2, len(vs))

print("== (c) a warehouse that does not exist; ListBuckets")
missing = "qd-tenant-0000000000000000000000000000000c"
expect("HEAD of a missing warehouse", (404, "404"), code(lambda: src.head_bucket(Bucket=missing)))
names = code(lambda: src.list_buckets())
info("ListBuckets by the admin user", names)
try:
    info("buckets listed", [b["Name"] for b in src.list_buckets()["Buckets"]])
except Exception as e:
    info("buckets listed", type(e).__name__)
info("HEAD of a bucket outside qd-tenant-*", code(lambda: src.head_bucket(Bucket="qd-backups")))

print("== (d) the remote target again")
r = admin(SOURCE, ADMIN_S, "PUT", f"/rustfs/admin/v3/set-remote-target?bucket={B}", target)
expect("set again: the same ARN", (200, ARN), (r.status_code, r.text.strip().strip('"')))
r = admin(SOURCE, ADMIN_S, "GET", f"/rustfs/admin/v3/list-remote-targets?bucket={B}&type=replication")
listed = r.json() or []
expect("one target listed", 1, len(listed))
info("its fields", sorted(listed[0]) if listed else None)
expect("listed with its ARN and endpoint", (ARN, "replica:9000"),
       (listed[0].get("arn"), listed[0].get("endpoint")) if listed else None)
info("its credentials listed", {k: ("…" if v else v) for k, v in (listed[0].get("credentials") or {}).items()}
     if listed else None)
other = {**target, "credentials": {"accessKey": ADMIN_R[0], "secretKey": "0" * 64}}
r = admin(SOURCE, ADMIN_S, "PUT", f"/rustfs/admin/v3/set-remote-target?bucket={B}", other)
info("set with a wrong secret for the replica", f"{r.status_code} {r.text[:160]}")


def arrives(key, within):
    for _ in range(within):
        try:
            dst.head_object(Bucket=B, Key=key)
            return True
        except Exception:
            time.sleep(1)
    return False


src.put_object(Bucket=B, Key="after-wrong.txt", Body=b"synthetic wrong")
info("an object written after the target was set with a wrong secret reaches the replica",
     arrives("after-wrong.txt", 20))
info("its source status", src.head_object(Bucket=B, Key="after-wrong.txt").get("ReplicationStatus"))
r = admin(SOURCE, ADMIN_S, "PUT", f"/rustfs/admin/v3/set-remote-target?bucket={B}", target)
info("set again with the right secret", r.status_code)
src.put_object(Bucket=B, Key="after-right.txt", Body=b"synthetic right")
expect("an object written after the right secret is set again reaches the replica", True,
       arrives("after-right.txt", 30))

print()
print("ALL OK" if not FAILED else f"SOME CHECKS FAILED: {len(FAILED)}")
raise SystemExit(1 if FAILED else 0)
