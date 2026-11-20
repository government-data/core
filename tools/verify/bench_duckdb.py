"""Benchmark for decision 0018's threshold: how a DuckDB worker of a given
memory behaves as the data grows past it. Synthetic client-like rows.

    python tools/verify/bench_duckdb.py gen ROWS...      # one Parquet per size
    python tools/verify/bench_duckdb.py run MEM THREADS  # the workload on each

The 2026-10-30 run (results in bench_duckdb_results_2GB_4.json):

    gen 9000000 18000000 36000000 72000000 145000000 290000000
    run 2GB 4

It writes about 30 GB to ~/.cache/qd-bench; delete it afterwards.
"""
import os, sys, time, threading, json, glob, shutil
import duckdb

DIR = os.path.expanduser("~/.cache/qd-bench")
os.makedirs(DIR, exist_ok=True)

GEN = """
COPY (
  SELECT
    lpad(CAST(hash(i % {distinct}) % 10000000000 AS VARCHAR), 10, '0') AS doc,   -- 2 % repeat
    CASE WHEN hash(i, 1) % 10 = 0 THEN 'PJ' ELSE 'PN' END AS client_type,
    (['EMPLEADO','INDEPENDIENTE','PENSIONADO','HOGAR','ESTUDIANTE'])[CAST(1 + hash(i, 2) % 5 AS BIGINT)] AS activity,
    CASE WHEN hash(i, 3) % 13 = 0 THEN NULL ELSE 'EMPRESA ' || (hash(i, 4) % 50000) END AS employer,
    CASE WHEN hash(i, 5) % 17 = 0 THEN NULL ELSE '3' || lpad(CAST(hash(i, 6) % 1000000000 AS VARCHAR), 9, '0') END AS mobile,
    CASE WHEN hash(i, 7) % 19 = 0 THEN 'sin correo' ELSE 'u' || hash(i, 8) || '@example.org' END AS email,
    lpad(CAST(5000000 + hash(i, 9) % 999 AS VARCHAR), 8, '0') AS city,
    CAST(1000 + hash(i, 10) % 9000 AS VARCHAR) AS ciiu,
    (1000000 + hash(i, 11) % 9000000)::DOUBLE AS income,
    (500000 + hash(i, 12) % 9000000)::DOUBLE AS expenses,
    CAST(hash(i, 13) % 50000000 AS BIGINT)::DOUBLE AS assets,
    CAST(hash(i, 14) % 20000000 AS BIGINT)::DOUBLE AS liabilities,
    (CAST(hash(i, 13) % 50000000 AS BIGINT) - CAST(hash(i, 14) % 20000000 AS BIGINT) + CASE WHEN hash(i, 15) % 23 = 0 THEN 5 ELSE 0 END)::DOUBLE AS net_worth,
    DATE '2015-01-01' + CAST(hash(i, 16) % 4000 AS INTEGER) AS updated,
    DATE '1950-01-01' + CAST(hash(i, 17) % 25000 AS INTEGER) AS birth,
    1 + hash(i, 18) % 6 AS stratum
  FROM range({rows}) t(i)
) TO '{path}' (FORMAT parquet, COMPRESSION zstd, ROW_GROUP_SIZE 1000000)
"""

RULES = """
WITH m AS (
  SELECT doc,
    CASE WHEN client_type = 'PN' AND activity = 'EMPLEADO' THEN employer IS NULL OR trim(employer) = '' END AS r01,
    CASE WHEN TRUE THEN mobile IS NULL OR NOT regexp_matches(mobile, '^3[0-9]{{9}}$') END AS r02,
    CASE WHEN TRUE THEN email IS NULL OR NOT regexp_matches(email, '^[^@ ]+@[^@ ]+\\.[a-z]{{2,}}$') END AS r03,
    CASE WHEN TRUE THEN NOT regexp_matches(city, '^[0-9]{{8}}$') END AS r04,
    CASE WHEN TRUE THEN length(ciiu) <> 4 END AS r05,
    CASE WHEN TRUE THEN abs(assets - liabilities - net_worth) > 1 END AS r06,
    CASE WHEN income > 0 THEN expenses > 1.5 * income END AS r07,
    CASE WHEN TRUE THEN net_worth < 0 END AS r08,
    CASE WHEN client_type = 'PN' THEN date_diff('day', birth, DATE '2026-06-30') / 365.25 NOT BETWEEN 0 AND 110 END AS r09,
    CASE WHEN client_type = 'PN' THEN stratum NOT BETWEEN 1 AND 6 END AS r10,
    CASE WHEN TRUE THEN date_diff('day', updated, DATE '2026-06-30') > 365 END AS r11,
    CASE WHEN activity IN ('HOGAR', 'ESTUDIANTE') THEN employer IS NOT NULL END AS r12
  FROM read_parquet('{path}')
)
SELECT {aggs} FROM m
"""
AGGS = ", ".join(f"count(r{i:02d}) AS a{i:02d}, sum(r{i:02d}::INT) AS f{i:02d}" for i in range(1, 13))

UNIQUE = "SELECT count(*) FROM (SELECT doc FROM read_parquet('{path}') GROUP BY doc HAVING count(*) > 1)"

ICD = """
SELECT count(*), avg(icd) FROM (
  SELECT doc, avg(1 - (CASE WHEN mobile IS NULL THEN 1 ELSE 0 END + CASE WHEN employer IS NULL THEN 1 ELSE 0 END) / 2.0) AS icd
  FROM read_parquet('{path}') GROUP BY doc)
"""


def gen(rows_list):
    con = duckdb.connect()
    for rows in rows_list:
        rows = int(rows)
        path = f"{DIR}/clients_{rows}.parquet"
        if os.path.exists(path):
            continue
        t = time.time()
        con.execute(GEN.format(rows=rows, distinct=int(rows * 0.98), path=path))
        print(f"gen {rows:>12,} rows  {os.path.getsize(path)/1e9:7.2f} GB  {time.time()-t:6.1f}s", flush=True)


def dir_size(p):
    total = 0
    for root, _, files in os.walk(p):
        for f in files:
            try:
                total += os.path.getsize(os.path.join(root, f))
            except OSError:
                pass
    return total


def run(mem, threads):
    out = []
    files = sorted(glob.glob(f"{DIR}/clients_*.parquet"), key=os.path.getsize)
    for path in files:
        gb = os.path.getsize(path) / 1e9
        rows = int(path.rsplit("_", 1)[1].split(".")[0])
        tmp = f"{DIR}/spill"
        shutil.rmtree(tmp, ignore_errors=True)
        os.makedirs(tmp)
        con = duckdb.connect()
        con.execute(f"SET memory_limit='{mem}'; SET threads={threads}; SET temp_directory='{tmp}'; SET preserve_insertion_order=false;")
        row = {"rows": rows, "parquet_gb": round(gb, 2), "mem": mem, "threads": threads}
        for name, sql in (("rules", RULES.format(path=path, aggs=AGGS)), ("unique", UNIQUE.format(path=path)), ("icd", ICD.format(path=path))):
            peak = [0]
            stop = threading.Event()
            def watch():
                while not stop.is_set():
                    peak[0] = max(peak[0], dir_size(tmp))
                    time.sleep(0.2)
            th = threading.Thread(target=watch); th.start()
            t = time.time()
            try:
                con.execute(sql).fetchall()
                secs = time.time() - t
                row[name] = round(secs, 1)
            except Exception as e:
                row[name] = f"FAIL {type(e).__name__}: {str(e)[:80]}"
            stop.set(); th.join()
            row[name + "_spill_gb"] = round(peak[0] / 1e9, 2)
        con.close()
        out.append(row)
        print(json.dumps(row), flush=True)
    with open(f"{DIR}/results_{mem}_{threads}.json", "w") as f:
        json.dump(out, f, indent=1)


if __name__ == "__main__":
    if sys.argv[1] == "gen":
        gen(sys.argv[2:])
    else:
        run(sys.argv[2], int(sys.argv[3]))
