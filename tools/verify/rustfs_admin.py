"""Verification for decision 0028: what the application can configure in
RustFS through its APIs (admin, S3, Iceberg catalog), as an admin user of its
own instead of root. Test names and synthetic objects only; credentials from
the environment, never printed.

Against the dev stack (watch mode), from the repository root, with a second,
throwaway RustFS as "the other server" for replication:

    docker run -d --name qd-rustfs-target -p 127.0.0.1:9100:9000 \\
      -e RUSTFS_ACCESS_KEY=qdtarget -e RUSTFS_SECRET_KEY=qdtarget-secret-123 \\
      -e RUSTFS_VOLUMES=/data rustfs/rustfs:1.0.4
    docker network connect data-quality-edge qd-rustfs-target
    set -a; . ./.env; set +a
    /tmp/qd-verify/bin/python tools/verify/rustfs_admin.py   # boto3, requests
    docker rm -f qd-rustfs-target

It removes everything it creates (buckets, users, policies)."""
import hashlib, json, os, time
from pathlib import Path

import boto3, requests
from botocore.auth import S3SigV4Auth
from botocore.awsrequest import AWSRequest
from botocore.config import Config
from botocore.credentials import Credentials
from requests.adapters import HTTPAdapter

ROOT = (os.environ["RUSTFS_ACCESS_KEY"], os.environ["RUSTFS_SECRET_KEY"])
CA = os.environ.get("DEV_CA", str(Path(__file__).resolve().parents[2] / "infra/traefik/certs/dev-ca.crt"))
BASE = "https://localhost"
TARGET_URL, TARGET = "http://127.0.0.1:9100", ("qdtarget", "qdtarget-secret-123")
TARGET_FROM_PRIMARY = "qd-rustfs-target:9000"
PLATFORM = ("qd-platform-admin", "qd-platform-admin-secret-0123")
POOL = ("qd-pool-default", "qd-pool-default-secret-0123")
BUCKET, OTHER = "qd-tenant-demo", "qd-not-managed"
results = []


def step(name):
    def wrap(fn):
        t = time.time()
        try:
            results.append(("OK", name, fn(), time.time() - t))
        except Exception as e:
            results.append(("FAIL", name, f"{type(e).__name__}: {str(e)[:250]}", time.time() - t))
        return fn
    return wrap


class SigV4(HTTPAdapter):
    """RustFS wants x-amz-content-sha256 and content-length signed."""

    def __init__(self, creds):
        super().__init__()
        self.creds = Credentials(*creds)

    def add_headers(self, request, **kw):
        body = request.body or b""
        if isinstance(body, str):
            body = body.encode()
        request.headers["x-amz-content-sha256"] = hashlib.sha256(body).hexdigest()
        request.headers["content-length"] = str(len(body))
        request.headers.pop("connection", None)
        aws = AWSRequest(method=request.method, url=request.url, data=body, headers=dict(request.headers))
        S3SigV4Auth(self.creds, "s3", "us-east-1").add_auth(aws)
        request.headers.update(dict(aws.headers))


def api(creds, method, path, body=None):
    s = requests.Session()
    s.mount(BASE, SigV4(creds))
    data = json.dumps(body) if isinstance(body, (dict, list)) else (body or b"")
    r = s.request(method, BASE + path, data=data, verify=CA)
    if r.status_code >= 300:
        raise RuntimeError(f"HTTP {r.status_code}: {r.text[:200]}")
    return r


def s3(creds, url=BASE):
    return boto3.client("s3", endpoint_url=url, aws_access_key_id=creds[0], aws_secret_access_key=creds[1],
                        region_name="us-east-1", verify=CA if url.startswith("https") else True,
                        config=Config(s3={"addressing_style": "path"}, retries={"max_attempts": 1}))


def empty_and_delete(c, bucket):
    try:
        for v in c.list_object_versions(Bucket=bucket).get("Versions", []) + c.list_object_versions(Bucket=bucket).get("DeleteMarkers", []):
            c.delete_object(Bucket=bucket, Key=v["Key"], VersionId=v["VersionId"])
        for o in c.list_objects_v2(Bucket=bucket).get("Contents", []):
            c.delete_object(Bucket=bucket, Key=o["Key"])
        c.delete_bucket(Bucket=bucket)
    except Exception:
        pass


# What the platform's own admin user may do: everything on the tenants'
# buckets, and the admin API (users, policies, quotas, targets, info).
PLATFORM_POLICY = {"Version": "2012-10-17", "Statement": [
    {"Effect": "Allow", "Action": ["s3:*"], "Resource": ["arn:aws:s3:::qd-tenant-*", "arn:aws:s3:::qd-tenant-*/*"]},
    {"Effect": "Allow", "Action": ["admin:*"], "Resource": ["arn:aws:s3:::*"]},
]}
POOL_POLICY = {"Version": "2012-10-17", "Statement": [
    {"Effect": "Allow", "Action": ["s3:GetObject", "s3:PutObject", "s3:DeleteObject", "s3:ListBucket"],
     "Resource": [f"arn:aws:s3:::{BUCKET}", f"arn:aws:s3:::{BUCKET}/*"]},
]}


@step("root: AWS action names RustFS rejects")
def _():
    rejected = []
    for action in ["s3:PutLifecycleConfiguration", "admin:GetBucketQuota", "s3tables:*", "s3:PutBucketLifecycle"]:
        try:
            api(ROOT, "PUT", "/rustfs/admin/v3/add-canned-policy?name=qd-probe",
                {"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Action": [action], "Resource": ["arn:aws:s3:::*"]}]})
        except RuntimeError:
            rejected.append(action)
    return f"rejected: {rejected} (use s3:PutBucketLifecycle / s3:GetBucketLifecycle)"


@step("root, once: the platform's policy and admin user")
def _():
    api(ROOT, "PUT", "/rustfs/admin/v3/add-canned-policy?name=qd-platform-admin", PLATFORM_POLICY)
    api(ROOT, "PUT", f"/rustfs/admin/v3/add-user?accessKey={PLATFORM[0]}", {"secretKey": PLATFORM[1], "status": "enabled"})
    api(ROOT, "PUT", f"/rustfs/admin/v3/set-user-or-group-policy?policyName=qd-platform-admin&userOrGroup={PLATFORM[0]}&isGroup=false")
    return "created; from here on, root is not used"


@step("platform: a tenant's warehouse (bucket + table bucket)")
def _():
    s3(PLATFORM).create_bucket(Bucket=BUCKET)
    api(PLATFORM, "PUT", f"/iceberg/v1/buckets/{BUCKET}")
    state = api(PLATFORM, "GET", f"/iceberg/v1/buckets/{BUCKET}").json()
    return f"enabled={state.get('enabled')} catalog={state.get('catalog-type')}"


@step("platform: denied outside the tenants' buckets")
def _():
    try:
        s3(PLATFORM).create_bucket(Bucket=OTHER)
        return "NOT DENIED (unexpected)"
    except Exception as e:
        return f"denied ({type(e).__name__})"


@step("platform: a worker pool's user and policy")
def _():
    api(PLATFORM, "PUT", "/rustfs/admin/v3/add-canned-policy?name=qd-pool-default", POOL_POLICY)
    api(PLATFORM, "PUT", f"/rustfs/admin/v3/add-user?accessKey={POOL[0]}", {"secretKey": POOL[1], "status": "enabled"})
    api(PLATFORM, "PUT", f"/rustfs/admin/v3/set-user-or-group-policy?policyName=qd-pool-default&userOrGroup={POOL[0]}&isGroup=false")
    s3(POOL).put_object(Bucket=BUCKET, Key="pool-check.txt", Body=b"synthetic")
    try:
        s3(POOL).create_bucket(Bucket="qd-tenant-other")
        return "pool writes its warehouse, but was NOT denied a new bucket (unexpected)"
    except Exception:
        return "pool writes its warehouse; denied anything else"


@step("platform: lifecycle rules (retention by temperature)")
def _():
    c = s3(PLATFORM)
    c.put_bucket_lifecycle_configuration(Bucket=BUCKET, LifecycleConfiguration={"Rules": [
        {"ID": "hot-detail-13-months", "Status": "Enabled", "Filter": {"Prefix": "hot/"}, "Expiration": {"Days": 395}},
        {"ID": "audit-archive-10-years", "Status": "Enabled", "Filter": {"Prefix": "audit/"}, "Expiration": {"Days": 3653}},
    ]})
    return [(r["ID"], r["Expiration"]["Days"]) for r in c.get_bucket_lifecycle_configuration(Bucket=BUCKET)["Rules"]]


@step("platform: a hard quota on a new bucket (blocks writes in 1.0.4)")
def _():
    api(PLATFORM, "PUT", f"/rustfs/admin/v3/set-bucket-quota?bucket={BUCKET}", {"quota": 50 * 1024**3, "quotatype": "hard"})
    try:
        s3(PLATFORM).put_object(Bucket=BUCKET, Key="quota-check.txt", Body=b"synthetic")
        outcome = "writes work with a quota"
    except Exception as e:
        outcome = f"writes BLOCKED with a quota ({str(e)[-70:]})"
    api(PLATFORM, "PUT", f"/rustfs/admin/v3/set-bucket-quota?bucket={BUCKET}", {"quota": 0, "quotatype": "hard"})
    s3(PLATFORM).put_object(Bucket=BUCKET, Key="quota-check.txt", Body=b"synthetic")
    return outcome + "; quota cleared, writes work again"


@step("platform: lake health (disks online/offline)")
def _():
    b = api(PLATFORM, "GET", "/rustfs/admin/v3/info").json()["info"]["backend"]
    return f"{b['backendType']}: online={b['onlineDisks']} offline={b['offlineDisks']} sets={len(b['totalSets'])}"


@step("platform: replication to the other server")
def _():
    src, dst = s3(PLATFORM), s3(TARGET, TARGET_URL)
    dst.create_bucket(Bucket=BUCKET)
    for c in (src, dst):
        c.put_bucket_versioning(Bucket=BUCKET, VersioningConfiguration={"Status": "Enabled"})
    arn = api(PLATFORM, "PUT", f"/rustfs/admin/v3/set-remote-target?bucket={BUCKET}", {
        "endpoint": TARGET_FROM_PRIMARY, "credentials": {"accessKey": TARGET[0], "secretKey": TARGET[1]},
        "targetbucket": BUCKET, "secure": False, "type": "replication", "region": "us-east-1"}).text.strip().strip('"')
    src.put_bucket_replication(Bucket=BUCKET, ReplicationConfiguration={"Role": arn, "Rules": [
        {"ID": "to-other-server", "Status": "Enabled", "Priority": 1, "Filter": {"Prefix": ""},
         "DeleteMarkerReplication": {"Status": "Enabled"}, "Destination": {"Bucket": arn}}]})
    src.put_object(Bucket=BUCKET, Key="replication-check.txt", Body=b"synthetic")
    for i in range(30):
        try:
            dst.head_object(Bucket=BUCKET, Key="replication-check.txt")
            return f"replicated in {i} s"
        except Exception:
            time.sleep(1)
    return "NOT replicated within 30 s"


@step("clean up")
def _():
    src = s3(ROOT)
    try:
        src.delete_bucket_replication(Bucket=BUCKET)
    except Exception:
        pass
    empty_and_delete(src, BUCKET)
    empty_and_delete(s3(TARGET, TARGET_URL), BUCKET)
    for user in (POOL[0], PLATFORM[0]):
        # DELETE needs a body: without one, RustFS rejects the signature.
        api(ROOT, "DELETE", f"/rustfs/admin/v3/remove-user?accessKey={user}", {})
    for name in ("qd-pool-default", "qd-platform-admin", "qd-probe"):
        try:
            api(ROOT, "DELETE", f"/rustfs/admin/v3/remove-canned-policy?name={name}", {})
        except RuntimeError:
            pass
    left = [b["Name"] for b in src.list_buckets()["Buckets"] if b["Name"].startswith("qd-tenant")]
    return f"tenant buckets left: {left}"


w = max(len(r[1]) for r in results)
for status, name, out, secs in results:
    print(f"[{status:4}] {name:<{w}}  {secs:5.1f}s  {out}")
