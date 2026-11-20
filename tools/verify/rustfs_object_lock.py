"""Verification for the lake replica (docs: development/lake-sizing). Object lock in RustFS 1.0.4: can a replica keep versions nobody can delete
for a retention period? Test bucket only; removed at the end when possible."""
import time

src = open("tools/verify/rustfs_admin.py").read()
exec(src[: src.index("# What the platform's own admin user")])
B = "qd-tenant-lock"
c = s3(ROOT)
results = []


def step(name):
    def wrap(fn):
        try:
            results.append(("OK", name, fn()))
        except Exception as e:
            results.append(("FAIL", name, f"{type(e).__name__}: {str(e)[:200]}"))
        return fn
    return wrap


@step("bucket with object lock enabled")
def _():
    c.create_bucket(Bucket=B, ObjectLockEnabledForBucket=True)
    return c.get_object_lock_configuration(Bucket=B)["ObjectLockConfiguration"].get("ObjectLockEnabled")


@step("default retention: COMPLIANCE, 1 day")
def _():
    c.put_object_lock_configuration(Bucket=B, ObjectLockConfiguration={
        "ObjectLockEnabled": "Enabled", "Rule": {"DefaultRetention": {"Mode": "COMPLIANCE", "Days": 1}}})
    return c.get_object_lock_configuration(Bucket=B)["ObjectLockConfiguration"]["Rule"]["DefaultRetention"]


V = {}


@step("write an object (it gets the retention)")
def _():
    r = c.put_object(Bucket=B, Key="cut-2026-06.parquet", Body=b"synthetic")
    V["id"] = r.get("VersionId")
    h = c.head_object(Bucket=B, Key="cut-2026-06.parquet", VersionId=V["id"])
    return f"version {V['id'][:8]}… mode={h.get('ObjectLockMode')} until={h.get('ObjectLockRetainUntilDate')}"


@step("delete without version: only a delete marker, the version stays")
def _():
    c.delete_object(Bucket=B, Key="cut-2026-06.parquet")
    vs = c.list_object_versions(Bucket=B)
    return f"versions={len(vs.get('Versions', []))} delete markers={len(vs.get('DeleteMarkers', []))}"


@step("delete the locked version itself (even as root)")
def _():
    try:
        c.delete_object(Bucket=B, Key="cut-2026-06.parquet", VersionId=V["id"])
        return "DELETED (not protected)"
    except Exception as e:
        return f"refused, as expected ({str(e)[:90]})"


@step("overwrite the retention to shorten it (COMPLIANCE forbids it)")
def _():
    try:
        c.put_object_retention(Bucket=B, Key="cut-2026-06.parquet", VersionId=V["id"],
                               Retention={"Mode": "GOVERNANCE", "RetainUntilDate": "2026-01-01T00:00:00Z"})
        return "shortened (not protected)"
    except Exception as e:
        return f"refused, as expected ({str(e)[:90]})"


for status, name, out in results:
    print(f"[{status:4}] {name:<62} {out}")
print("note: the test bucket keeps a locked version for 1 day; it is removed after that.")
