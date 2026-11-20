# /// script
# requires-python = ">=3.11"
# dependencies = ["requests"]
# ///
"""A synthetic source for development: a PostgreSQL on 127.0.0.1:55432 with
the tables the `verify_*` datasets map (`public.clients`, `public.loans`)
and a few no mapping reads (`acct.ledger`, `public.audit_log`), every row
made up here (no person, no cooperative's data), with some faults for the
rules to find. The source `verify_pg` is pointed at it, its credentials
new each run (never printed, never written down). Then its tables are
listed again, and the lake's.

From the repository root, with the dev stack and the worker up:

    uv run tools/dev/synthetic_source.py
"""
import secrets
import subprocess
import time
from pathlib import Path

import requests
import urllib3

urllib3.disable_warnings()
ROOT = Path(__file__).resolve().parents[2]
env = dict(l.split("=", 1) for l in (ROOT / ".env").read_text().splitlines() if "=" in l and not l.startswith("#"))
CONTAINER, PORT, SOURCE = "data-quality-synthetic-pg", 55432, "verify_pg"
API = "https://localhost/api"

# Rows made up with generate_series: documents are digits, emails at
# example.test, every fault on purpose (blank, U+00A0, malformed, negative).
SQL = r"""
CREATE TABLE public.clients (id integer PRIMARY KEY, document varchar(20), email text,
                             opened date, notes text);
INSERT INTO public.clients
SELECT g,
       CASE WHEN g % 50 = 0 THEN '' WHEN g % 77 = 0 THEN E' ' ELSE lpad((10000000 + g * 37)::text, 10, '0') END,
       CASE WHEN g % 40 = 0 THEN 'client' || g || '@' ELSE 'client' || g || '@example.test' END,
       CASE WHEN g % 60 = 0 THEN NULL ELSE date '2015-01-01' + (g * 3) % 3650 END,
       CASE WHEN g % 3 = 0 THEN 'synthetic note ' || g END
FROM generate_series(1, 500) g;

CREATE TABLE public.loans (loan_id integer PRIMARY KEY, document varchar(20), email text,
                           mobile varchar(15), amount numeric(14,2), granted date,
                           active boolean, branch_code varchar(5));
INSERT INTO public.loans
SELECT g,
       lpad((10000000 + (g % 500 + 1) * 37)::text, 10, '0'),
       CASE WHEN g % 45 = 0 THEN 'loan' || g || '.example.test' ELSE 'loan' || g || '@example.test' END,
       CASE WHEN g % 30 = 0 THEN NULL WHEN g % 55 = 0 THEN '12345' ELSE '3' || lpad((g * 7919 % 1000000000)::text, 9, '0') END,
       CASE WHEN g % 70 = 0 THEN -100.00 ELSE round((g * 1373 % 5000000)::numeric + 100000, 2) END,
       CASE WHEN g % 90 = 0 THEN date '2031-01-01' ELSE date '2018-01-01' + (g * 5) % 2900 END,
       g % 9 <> 0,
       'B' || lpad((g % 12)::text, 3, '0')
FROM generate_series(1, 2000) g;

CREATE SCHEMA acct;
CREATE TABLE acct.ledger (entry_id bigint PRIMARY KEY, loan_id integer, amount numeric(14,2),
                          posted_at timestamp);
INSERT INTO acct.ledger
SELECT g, g % 2000 + 1, round((g * 911 % 100000)::numeric / 100, 2),
       timestamp '2024-01-01' + make_interval(hours => g)
FROM generate_series(1, 1000) g;

CREATE TABLE public.audit_log (id bigint PRIMARY KEY, action text, at timestamp);
INSERT INTO public.audit_log
SELECT g, (ARRAY['create','update','delete'])[g % 3 + 1], timestamp '2025-01-01' + make_interval(mins => g)
FROM generate_series(1, 300) g;
"""


def run(*args, **kw):
    return subprocess.run(list(args), check=True, capture_output=True, text=True, **kw)


def start(password: str):
    subprocess.run(["docker", "rm", "-f", CONTAINER], capture_output=True)
    run("docker", "run", "-d", "--restart", "unless-stopped", "--name", CONTAINER,
        "-e", f"POSTGRES_PASSWORD={password}", "-e", "POSTGRES_DB=synthetic",
        "-p", f"127.0.0.1:{PORT}:5432", "postgres:17-alpine")
    for _ in range(60):
        if subprocess.run(["docker", "exec", CONTAINER, "pg_isready", "-U", "postgres", "-d", "synthetic"],
                          capture_output=True).returncode == 0:
            break
        time.sleep(1)
    time.sleep(2)
    run("docker", "exec", "-i", CONTAINER, "psql", "-qv", "ON_ERROR_STOP=1", "-U", "postgres", "-d", "synthetic",
        input=SQL)


def counts():
    q = ("SELECT table_schema || '.' || table_name FROM information_schema.tables "
         "WHERE table_schema IN ('public','acct') ORDER BY 1")
    tables = run("docker", "exec", CONTAINER, "psql", "-qtAU", "postgres", "-d", "synthetic", "-c", q).stdout.split()
    for t in tables:
        n = run("docker", "exec", CONTAINER, "psql", "-qtAU", "postgres", "-d", "synthetic", "-c",
                f"SELECT count(*) FROM {t}").stdout.strip()
        print(f"  {t}: {n} rows")


def point_source(password: str):
    s = requests.Session()
    s.verify = False
    r = s.post(f"{API}/auth/login",
               json={"email": env["ADMIN_BOOTSTRAP_EMAIL"], "password": env["ADMIN_BOOTSTRAP_PASSWORD"]})
    r.raise_for_status()
    csrf = {"x-csrf-token": requests.utils.unquote(s.cookies.get("csrf", ""))}
    body = {"driver": "postgres",
            "fields": {"host": "127.0.0.1", "port": str(PORT), "database": "synthetic"},
            "secrets": {"user": "postgres", "password": password}}
    r = s.put(f"{API}/quality/sources/{SOURCE}", json=body, headers=csrf)
    print(f"source {SOURCE}: {r.status_code}")
    r.raise_for_status()
    print("list its tables:", s.post(f"{API}/quality/sources/{SOURCE}/tables", headers=csrf).status_code)
    print("list the lake:", s.post(f"{API}/quality/lake/tables", headers=csrf).status_code)


if __name__ == "__main__":
    password = secrets.token_urlsafe(24)
    start(password)
    print(f"{CONTAINER} on 127.0.0.1:{PORT}, synthetic rows:")
    counts()
    point_source(password)
