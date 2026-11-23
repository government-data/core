# /// script
# requires-python = ">=3.11"
# dependencies = ["pyiceberg[pyarrow]>=0.9", "requests", "botocore", "pyyaml"]
# ///
"""Loads one cut of local Parquet files into the dev lake's bronze layer
(decision 0017): one namespace per dataset (`<dataset>_bronze`), one table
per source file, the columns as extracted plus `cut`, partitioned by `cut`.
Which datasets, from their mappings alone, as a job finds them: each
mapping in `--mappings` names its dataset and its bronze table (`source`),
read from `<dir>/<source>.parquet`; `--load DATASET[.TABLE]=FILE` adds one
without a mapping. No dataset is named here. Loading a cut again replaces
that cut only (one snapshot), so it can run twice. Run it through
tools/dev/lake-load.sh, which sets the credentials. Prints counts only,
never values."""
import argparse, datetime, hashlib, os, sys
from pathlib import Path

import pyarrow as pa
import pyarrow.parquet as pq
import requests
import yaml
from botocore.auth import S3SigV4Auth
from botocore.awsrequest import AWSRequest
from botocore.credentials import Credentials
from pyiceberg.catalog.rest import RestCatalog
from pyiceberg.expressions import EqualTo
from pyiceberg.transforms import IdentityTransform

ROOT = Path(__file__).resolve().parents[2]
AK, SK = os.environ["LAKE_LOADER_ACCESS_KEY"], os.environ["LAKE_LOADER_SECRET_KEY"]
CA = os.environ.get("DEV_CA", str(ROOT / "infra/traefik/certs/dev-ca.crt"))
os.environ["SSL_CERT_FILE"] = CA
os.environ["REQUESTS_CA_BUNDLE"] = CA

# Arrow's S3 client ignores SSL_CERT_FILE and PyIceberg does not pass
# `tls_ca_file_path`: give it the development CA here.
import pyarrow.fs as _fs

_S3FileSystem = _fs.S3FileSystem
_fs.S3FileSystem = lambda **kw: _S3FileSystem(tls_ca_file_path=CA, **kw)


class SigV4Adapter(requests.adapters.HTTPAdapter):
    """RustFS wants x-amz-content-sha256 on every signed catalog request."""

    def add_headers(self, request, **kwargs):
        body = request.body or b""
        if isinstance(body, str):
            body = body.encode()
        request.headers["x-amz-content-sha256"] = hashlib.sha256(body).hexdigest()
        request.headers.pop("connection", None)
        # Traefik drops `Content-Length: 0` on a request with no body
        # (DELETE): signed, RustFS would miss it. Signed only with a body.
        if body:
            request.headers["content-length"] = str(len(body))
        else:
            request.headers.pop("content-length", None)
            request.headers.pop("Content-Length", None)
        aws = AWSRequest(method=request.method, url=request.url, data=body, headers=dict(request.headers))
        S3SigV4Auth(Credentials(AK, SK), "s3", "us-east-1").add_auth(aws)
        request.headers.update(dict(aws.headers))


class RustFSCatalog(RestCatalog):
    def _init_sigv4(self, session):
        session.mount(self.uri, SigV4Adapter())


def catalog(warehouse):
    return RustFSCatalog("rustfs", **{
        "uri": "https://localhost/iceberg",
        "warehouse": warehouse,
        "rest.sigv4-enabled": "true",
        "rest.signing-region": "us-east-1",
        "rest.signing-name": "s3",
        "py-io-impl": "pyiceberg.io.pyarrow.PyArrowFileIO",
        "s3.endpoint": "https://localhost",
        "s3.access-key-id": AK,
        "s3.secret-access-key": SK,
        "s3.region": "us-east-1",
        "s3.force-virtual-addressing": "false",
    })


def with_cut(table, cut):
    # Iceberg keeps microseconds: the extract's millisecond timestamps widen.
    fields = [pa.field(f.name, pa.timestamp("us")) if pa.types.is_timestamp(f.type) else f for f in table.schema]
    table = table.cast(pa.schema(fields))
    return table.append_column(pa.field("cut", pa.date32(), nullable=False), pa.array([cut] * table.num_rows, pa.date32()))


def load(cat, namespace, name, path, cut):
    data = with_cut(pq.read_table(path), cut)
    if (namespace,) not in cat.list_namespaces():
        cat.create_namespace(namespace)
    ident = (namespace, name)
    # RustFS 1.0.4 answers 403, not 404, to HEAD on a missing table: list instead.
    if ident not in cat.list_tables(namespace):
        tbl = cat.create_table(ident, schema=data.schema)
        with tbl.update_spec() as spec:
            spec.add_field("cut", IdentityTransform(), "cut")
    tbl = cat.load_table(ident)
    tbl.overwrite(data, overwrite_filter=EqualTo("cut", cut.isoformat()))
    rows = tbl.scan(row_filter=EqualTo("cut", cut.isoformat()), selected_fields=("cut",)).to_arrow().num_rows
    print(f"{namespace}.{name}: cut {cut} rows {rows} (source {data.num_rows}), columns {len(data.schema)}")
    return rows == data.num_rows


def tables(mappings, directory, extra):
    """(namespace, table, file) to load: the mappings', then `--load`'s."""
    found = []
    for path in sorted(Path(mappings).glob("*.yaml")):
        m = yaml.safe_load(path.read_text()) or {}
        if m.get("dataset") and m.get("source"):
            found.append((f"{m['dataset']}_bronze", m["source"], Path(directory) / f"{m['source']}.parquet"))
    for item in extra:
        name, _, file = item.partition("=")
        dataset, _, table = name.partition(".")
        if not dataset or not file:
            sys.exit(f"--load {item}: DATASET[.TABLE]=FILE")
        found.append((f"{dataset}_bronze", table or dataset, Path(file)))
    return found


def main():
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--cut", required=True, type=datetime.date.fromisoformat)
    p.add_argument("--dir", default=str(ROOT / "tmp/datos"), help="where <source>.parquet files are")
    p.add_argument("--mappings", default=str(ROOT / "examples/datasets"), help="a directory of mapping files")
    p.add_argument("--load", action="append", default=[], metavar="DATASET[.TABLE]=FILE",
                   help="a table without a mapping; repeatable")
    p.add_argument("--warehouse", default=os.environ.get("LAKE_WAREHOUSE", "qd-tenant-00000000000000000000000000000001"))
    args = p.parse_args()
    cat = catalog(args.warehouse)
    ok = True
    for namespace, name, path in tables(args.mappings, args.dir, args.load):
        if not path.exists():
            print(f"{namespace}.{name}: {path.name} not found, skipped")
            continue
        ok &= load(cat, namespace, name, path, args.cut)
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
