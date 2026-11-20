"""The checks of tools/verify/rustfs_replication_app.sh, after each run of
the application's adapter.

Reads both lakes through their gates with each lake's admin keys and the
`app` machine's certificate. MODE=first: what the run configured, and an
object written to the main lake arriving locked on the replica; the
configuration is written to snapshot.json. MODE=second: the configuration
is the same as after the first run, one target, and replication goes on.
Prints [OK  ]/[FAIL]; no key is printed.
"""

import hashlib
import json
import os
import time
from datetime import datetime, timezone

import boto3
import requests
from botocore.auth import S3SigV4Auth
from botocore.awsrequest import AWSRequest
from botocore.config import Config
from botocore.credentials import Credentials

D, MODE = os.environ["D"], os.environ["MODE"]
CA, APP = f"{D}/ca.crt", (f"{D}/app-1.crt", f"{D}/app-1.key")
SOURCE, REPLICA = os.environ["SOURCE"], os.environ["REPLICA"]
B = os.environ["WAREHOUSE"]
FAILED = []


def keys(name):
    d = dict(line.strip().split("=", 1) for line in open(f"{D}/{name}") if "=" in line)
    return d["ADMIN_ACCESS_KEY"], d["ADMIN_SECRET_KEY"]


ADMIN_S, ADMIN_R = keys("admin-source.env"), keys("admin-replica.env")


def expect(name, wanted, got):
    print(f"[OK  ] {name}: {got}" if wanted == got else f"[FAIL] {name}: expected {wanted!r}, got {got!r}")
    if wanted != got:
        FAILED.append(name)


def admin(base, creds, path):
    headers = {"x-amz-content-sha256": hashlib.sha256(b"").hexdigest()}
    aws = AWSRequest(method="GET", url=base + path, data=b"", headers=headers)
    S3SigV4Auth(Credentials(*creds), "s3", "us-east-1").add_auth(aws)
    return requests.get(base + path, headers=dict(aws.headers), verify=CA, cert=APP, timeout=30)


def s3(base, creds):
    return boto3.client("s3", endpoint_url=base, aws_access_key_id=creds[0], aws_secret_access_key=creds[1],
                        region_name="us-east-1", verify=CA,
                        config=Config(s3={"addressing_style": "path"}, retries={"max_attempts": 1},
                                      client_cert=APP, connect_timeout=5, read_timeout=30))


src, dst = s3(SOURCE, ADMIN_S), s3(REPLICA, ADMIN_R)


def configuration():
    targets = admin(SOURCE, ADMIN_S, f"/rustfs/admin/v3/list-remote-targets?bucket={B}&type=replication").json()
    rule = src.get_bucket_replication(Bucket=B)["ReplicationConfiguration"]["Rules"]
    return {
        "lock": dst.get_object_lock_configuration(Bucket=B)["ObjectLockConfiguration"],
        "replica_versioning": dst.get_bucket_versioning(Bucket=B).get("Status"),
        "versioning": src.get_bucket_versioning(Bucket=B).get("Status"),
        "lifecycle": src.get_bucket_lifecycle_configuration(Bucket=B)["Rules"],
        "rule": rule,
        "targets": [{k: t.get(k) for k in ("arn", "endpoint", "secure", "targetbucket")} for t in targets or []],
    }


def arrives(key):
    src.put_object(Bucket=B, Key=key, Body=b"synthetic " + key.encode())
    for _ in range(30):
        try:
            return dst.head_object(Bucket=B, Key=key)
        except Exception:
            time.sleep(1)
    return None


now = configuration()
if MODE == "first":
    expect("replica: default retention COMPLIANCE 30 days", {"Mode": "COMPLIANCE", "Days": 30},
           now["lock"].get("Rule", {}).get("DefaultRetention"))
    expect("replica: versioning (with the object lock)", "Enabled", now["replica_versioning"])
    expect("main lake: versioning", "Enabled", now["versioning"])
    expect("main lake: noncurrent versions expire after 7 days", (1, 7),
           (len(now["lifecycle"]), now["lifecycle"][0].get("NoncurrentVersionExpiration", {}).get("NoncurrentDays")))
    expect("main lake: one remote target, through its proxy, secure", [("lake-1:9100", True)],
           [(t["endpoint"], t["secure"]) for t in now["targets"]])
    expect("main lake: the rule, without delete markers", ("qd-replica", "Disabled"),
           (now["rule"][0].get("ID"), now["rule"][0].get("DeleteMarkerReplication", {}).get("Status")))
    h = arrives("first.txt")
    expect("an object written to the main lake arrives on the replica", True, h is not None)
    if h:
        expect("as a replica", "REPLICA", h.get("ReplicationStatus"))
        expect("locked COMPLIANCE", "COMPLIANCE", h.get("ObjectLockMode"))
        days = (h["ObjectLockRetainUntilDate"] - datetime.now(timezone.utc)).days
        expect("retained about 30 days", True, 29 <= days <= 30)
        vid = h.get("VersionId")
        try:
            dst.delete_object(Bucket=B, Key="first.txt", VersionId=vid)
            deleted = "deleted"
        except Exception:
            deleted = "refused"
        expect("its version refused to the replica's admin user's delete", "refused", deleted)
    json.dump(now, open(f"{D}/snapshot.json", "w"), default=str, sort_keys=True)
else:
    before = json.load(open(f"{D}/snapshot.json"))
    expect("a second run changed nothing in either lake's configuration", before,
           json.loads(json.dumps(now, default=str, sort_keys=True)))
    expect("still one remote target", 1, len(now["targets"]))
    expect("an object written after the second run arrives", True, arrives("second.txt") is not None)

print("ALL OK" if not FAILED else f"SOME CHECKS FAILED: {len(FAILED)}")
raise SystemExit(1 if FAILED else 0)
