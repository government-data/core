# /// script
# requires-python = ">=3.11"
# dependencies = ["boto3"]
# ///
"""Verification of what browsing RustFS's console costs (decision 0028): how
long a folder's listing (`ListObjectsV2` with `delimiter=/`, what each click
in the console sends) takes as the objects under it grow, in a plain bucket
and in a table bucket (one enabled for the Iceberg catalog, as every tenant
warehouse is), and what caching headers the console's static files carry.
One image after another, so a newer version can be compared with the one of
infra/rustfs.

What it shows (1.0.4 and 1.0.5): in a table bucket RustFS answers a listing
with a delimiter by walking every object under the prefix, 1000 at a time
and without the delimiter, and folding the common prefixes itself
(`rustfs/src/app/table_list_isolation.rs`), so a folder of two entries
costs as much as its whole subtree; a plain bucket's listing does not. Past
16 raw pages (16000 objects under the prefix) it stops, with a continuation
token or an error, which the last check reports.

Each image runs as its own throwaway RustFS, with random root credentials
that never leave this process; nothing of the dev stack is touched. The
objects are synthetic: a tree shaped like an Iceberg warehouse
(`tables/<table>/data/part=<n>/<file>` and `tables/<table>/metadata/<file>`),
64 bytes each. From the repository root:

    uv run tools/verify/rustfs_listing.py [image ...]

Without arguments: rustfs/rustfs:1.0.4 and rustfs/rustfs:1.0.5.
"""
import hashlib, secrets, socket, statistics, subprocess, sys, time, urllib.request
from concurrent.futures import ThreadPoolExecutor

import boto3
from botocore.auth import S3SigV4Auth
from botocore.awsrequest import AWSRequest
from botocore.config import Config
from botocore.credentials import Credentials

IMAGES = sys.argv[1:] or ["rustfs/rustfs:1.0.4", "rustfs/rustfs:1.0.5"]
NAME = "qd-verify-rustfs-listing"
BUCKETS = {"qd-verify-plain": False, "qd-verify-table": True}  # name: table bucket
TABLES, PARTS = 10, 10  # per table: PARTS partitions and a metadata folder
SIZES = [500, 2000, 8000]  # objects in each bucket at each measurement
PAST_LIMIT = 17000  # objects under one prefix, past the table listing's 16 pages
RUNS = 5


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def key(i):
    table, rest = i % TABLES, i // TABLES
    if rest % (PARTS + 1) == PARTS:
        return f"tables/t{table:02d}/metadata/m{rest:06d}.avro"
    return f"tables/t{table:02d}/data/part={rest % (PARTS + 1):02d}/f{rest:06d}.parquet"


def start(image, s3_port, console_port):
    """The throwaway's S3 client and a signed PUT for its other APIs."""
    access, secret = "qd" + secrets.token_hex(6), secrets.token_hex(20)
    subprocess.run(["docker", "rm", "-f", "-v", NAME], capture_output=True)
    subprocess.run(
        ["docker", "run", "-d", "--name", NAME, "--tmpfs", "/logs",
         "-p", f"127.0.0.1:{s3_port}:9000", "-p", f"127.0.0.1:{console_port}:9001",
         "-e", f"RUSTFS_ACCESS_KEY={access}", "-e", f"RUSTFS_SECRET_KEY={secret}",
         "-e", "RUSTFS_VOLUMES=/data", "-e", "RUSTFS_OBS_LOGGER_LEVEL=warn",
         image, "server", "--address", ":9000", "--console-enable", "/data"],
        check=True, capture_output=True)
    base = f"http://127.0.0.1:{s3_port}"
    s3 = boto3.client(
        "s3", endpoint_url=base, region_name="us-east-1",
        aws_access_key_id=access, aws_secret_access_key=secret,
        config=Config(s3={"addressing_style": "path"}, max_pool_connections=32,
                      retries={"max_attempts": 1}))

    def put(path):
        headers = {"x-amz-content-sha256": hashlib.sha256(b"").hexdigest()}
        req = AWSRequest(method="PUT", url=base + path, data=b"", headers=headers)
        S3SigV4Auth(Credentials(access, secret), "s3", "us-east-1").add_auth(req)
        call = urllib.request.Request(base + path, data=b"", method="PUT", headers=dict(req.headers))
        return urllib.request.urlopen(call, timeout=30).status

    for _ in range(60):
        try:
            s3.list_buckets()
            return s3, put
        except Exception:
            time.sleep(1)
    raise SystemExit(f"{image}: did not start")


def make_buckets(s3, put):
    for bucket, table in BUCKETS.items():
        s3.create_bucket(Bucket=bucket)
        if table:  # as a warehouse is made: the catalog enabled on the bucket
            put(f"/iceberg/v1/buckets/{bucket}")


def load(s3, bucket, start_at, end):
    body = b"x" * 64
    with ThreadPoolExecutor(32) as pool:
        list(pool.map(lambda i: s3.put_object(Bucket=bucket, Key=key(i), Body=body),
                      range(start_at, end)))


def timed(fn):
    """Median milliseconds of RUNS calls, and the entries the last returned."""
    took, entries = [], 0
    for _ in range(RUNS):
        t = time.perf_counter()
        r = fn()
        took.append(1000 * (time.perf_counter() - t))
        entries = len(r.get("Contents", [])) + len(r.get("CommonPrefixes", []))
    return statistics.median(took), entries


def listings(s3, bucket):
    one = lambda **kw: (lambda: s3.list_objects_v2(Bucket=bucket, **kw))
    return [
        ("root, delimiter", one(Delimiter="/")),
        ("tables/, delimiter", one(Prefix="tables/", Delimiter="/")),
        ("one table, delimiter", one(Prefix="tables/t00/", Delimiter="/")),
        ("one partition, delimiter", one(Prefix="tables/t00/data/part=00/", Delimiter="/")),
        ("flat, 1000 keys", one(MaxKeys=1000)),
    ]


def past_the_limit(s3, bucket):
    """A root listing with a delimiter, followed to its end, once the bucket
    holds PAST_LIMIT objects: pages, entries and time, or the error."""
    pages, entries, token, t = 0, 0, None, time.perf_counter()
    try:
        while True:
            kw = {"ContinuationToken": token} if token else {}
            r = s3.list_objects_v2(Bucket=bucket, Delimiter="/", **kw)
            pages += 1
            entries += len(r.get("Contents", [])) + len(r.get("CommonPrefixes", []))
            token = r.get("NextContinuationToken")
            if not r.get("IsTruncated") or not token or pages > 50:
                break
        ms = 1000 * (time.perf_counter() - t)
        return f"{pages} page(s), {entries} entries, {ms:.0f} ms"
    except Exception as e:
        code = getattr(e, "response", {}).get("Error", {}).get("Code", type(e).__name__)
        return f"error {code} after {pages} page(s)"


def console_headers(console_port):
    """The caching headers of the console's page and of one of its hashed scripts."""
    base = f"http://127.0.0.1:{console_port}"
    get = lambda path: urllib.request.urlopen(base + path, timeout=10)
    page = get("/rustfs/console/browser/")
    html = page.read().decode("utf-8", "replace")
    start_at = html.find("/rustfs/console/_next/static/")
    script = html[start_at:html.find('"', start_at)] if start_at >= 0 else None
    names = ("cache-control", "etag", "last-modified")
    show = lambda r: ", ".join(f"{n}: {r.headers.get(n) or '-'}" for n in names)
    print(f"  console page: {show(page)}")
    print(f"  hashed script: {show(get(script)) if script else 'none found'}")


def measure(image):
    print(f"\n== {image}")
    s3, put = start(image, free_port(), console_port := free_port())
    try:
        console_headers(console_port)
        make_buckets(s3, put)
        loaded = 0
        for size in SIZES + [PAST_LIMIT]:
            for bucket, table in BUCKETS.items():
                load(s3, bucket, loaded, size)
                kind = "table" if table else "plain"
                if size == PAST_LIMIT:
                    print(f"  {size:>5} objects | {kind} | root, delimiter, to the end | "
                          f"{past_the_limit(s3, bucket)}")
                    continue
                for label, fn in listings(s3, bucket):
                    ms, entries = timed(fn)
                    print(f"  {size:>5} objects | {kind} | {label:<25} | {entries:>4} entries | {ms:>7.0f} ms")
            loaded = size
    finally:
        subprocess.run(["docker", "rm", "-f", "-v", NAME], capture_output=True)


for image in IMAGES:
    measure(image)
