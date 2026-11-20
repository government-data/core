"""Verification for decision 0030. Can RustFS hand out temporary credentials (STS AssumeRole), scoped by a
session policy to one tenant's warehouse? Test names only; credentials from
the environment, never printed."""
import json, os, time
from datetime import datetime, timezone

src = open("tools/verify/rustfs_admin.py").read()
exec(src[: src.index("# What the platform's own admin user")])

ISSUER = ("qd-sts-issuer", "qd-sts-issuer-secret-0123")
results = []


def step(name):
    def wrap(fn):
        t = time.time()
        try:
            results.append(("OK", name, fn(), time.time() - t))
        except Exception as e:
            results.append(("FAIL", name, f"{type(e).__name__}: {str(e)[:220]}", time.time() - t))
        return fn
    return wrap


def sts(creds):
    return boto3.client("sts", endpoint_url=BASE, aws_access_key_id=creds[0], aws_secret_access_key=creds[1],
                        region_name="us-east-1", verify=CA)


TMP = {}


@step("setup: an issuer user allowed on qd-tenant-* (root, once)")
def _():
    pol = {"Version": "2012-10-17", "Statement": [
        {"Effect": "Allow", "Action": ["s3:*"], "Resource": ["arn:aws:s3:::qd-tenant-*", "arn:aws:s3:::qd-tenant-*/*"]}]}
    api(ROOT, "PUT", "/rustfs/admin/v3/add-canned-policy?name=qd-sts-issuer", pol)
    api(ROOT, "PUT", f"/rustfs/admin/v3/add-user?accessKey={ISSUER[0]}", {"secretKey": ISSUER[1], "status": "enabled"})
    api(ROOT, "PUT", f"/rustfs/admin/v3/set-user-or-group-policy?policyName=qd-sts-issuer&userOrGroup={ISSUER[0]}&isGroup=false")
    for b in ("qd-tenant-a", "qd-tenant-b"):
        try:
            s3(ROOT).create_bucket(Bucket=b)
        except Exception:
            pass
    return "ok"


@step("AssumeRole: 15-minute credentials, session policy = only qd-tenant-a")
def _():
    session_policy = {"Version": "2012-10-17", "Statement": [
        {"Effect": "Allow", "Action": ["s3:GetObject", "s3:PutObject", "s3:ListBucket"],
         "Resource": ["arn:aws:s3:::qd-tenant-a", "arn:aws:s3:::qd-tenant-a/*"]}]}
    r = sts(ISSUER).assume_role(RoleArn="arn:aws:iam::000000000000:role/qd-job", RoleSessionName="job-1234",
                                DurationSeconds=900, Policy=json.dumps(session_policy))
    c = r["Credentials"]
    TMP["creds"] = (c["AccessKeyId"], c["SecretAccessKey"], c["SessionToken"])
    left = (c["Expiration"] - datetime.now(timezone.utc)).total_seconds()
    return f"temporary key issued; expires in {left / 60:.0f} min; has a session token: {bool(c['SessionToken'])}"


def tmp_s3():
    ak, sk, tok = TMP["creds"]
    return boto3.client("s3", endpoint_url=BASE, aws_access_key_id=ak, aws_secret_access_key=sk, aws_session_token=tok,
                        region_name="us-east-1", verify=CA, config=Config(s3={"addressing_style": "path"}, retries={"max_attempts": 1}))


@step("temporary credentials: write to qd-tenant-a")
def _():
    tmp_s3().put_object(Bucket="qd-tenant-a", Key="sts-check.txt", Body=b"synthetic")
    return "allowed"


@step("temporary credentials: denied on qd-tenant-b (outside the session policy)")
def _():
    try:
        tmp_s3().put_object(Bucket="qd-tenant-b", Key="sts-check.txt", Body=b"synthetic")
        return "NOT DENIED (unexpected)"
    except Exception as e:
        return f"denied ({type(e).__name__})"


@step("temporary credentials: no admin API")
def _():
    s = requests.Session()
    ak, sk, tok = TMP["creds"]

    class Tok(SigV4):
        def add_headers(self, request, **kw):
            request.headers["x-amz-security-token"] = tok
            super().add_headers(request, **kw)

    s.mount(BASE, Tok((ak, sk)))
    r = s.get(BASE + "/rustfs/admin/v3/info", verify=CA)
    return f"admin info: HTTP {r.status_code}"


@step("AssumeRole: shorter than the minimum (60 s) is refused or clamped")
def _():
    try:
        r = sts(ISSUER).assume_role(RoleArn="arn:aws:iam::000000000000:role/qd-job", RoleSessionName="job-short", DurationSeconds=60)
        left = (r["Credentials"]["Expiration"] - datetime.now(timezone.utc)).total_seconds()
        return f"issued, expires in {left:.0f} s"
    except Exception as e:
        return f"refused ({str(e)[:90]})"


@step("clean up")
def _():
    for b in ("qd-tenant-a", "qd-tenant-b"):
        empty_and_delete(s3(ROOT), b)
    api(ROOT, "DELETE", f"/rustfs/admin/v3/remove-user?accessKey={ISSUER[0]}", {})
    api(ROOT, "DELETE", "/rustfs/admin/v3/remove-canned-policy?name=qd-sts-issuer", {})
    return [b["Name"] for b in s3(ROOT).list_buckets()["Buckets"]]


w = max(len(r[1]) for r in results)
for status, name, out, secs in results:
    print(f"[{status:4}] {name:<{w}}  {secs:5.1f}s  {out}")
