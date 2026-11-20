"""Checks for decision 0033: how the rule patterns must compile to SQL so they
mean the same in every engine. Synthetic rows only.

    uv run --no-project --with duckdb python tools/verify/rule_semantics.py

Each check prints what plain SQL does and what the compiled pattern does.
"""
import duckdb

con = duckdb.connect()
con.execute("""
CREATE TABLE t AS SELECT * FROM (VALUES
  (1, 'EMPLEADO',   'ACME SAS',   '3001234567', 'Medellín',        1234.0, 'PN', 'a@x.co'),
  (2, 'EMPLEADO',   NULL,         '300123',     'Medellín' || chr(160), NULL, 'PN', 'A@X.CO'),
  (3, NULL,         '',           NULL,         'ANTIOQUIA',       0.0,    'PJ', NULL),
  (4, 'PENSIONADO', '  ',         '3001234567', 'medellín',        12.5,   'PN', 'notiene@notiene.com'),
  (5, 'ACTUALIZAR', 'ACTUALIZAR', '3109876543', 'Bello',           5.0,    'PN', 'b@x.co')
) AS v(id, activity, employer, mobile, city, amount, client_type, email)
""")


def show(title, sql):
    print(f"\n## {title}\n{sql.strip()}")
    for row in con.execute(sql).fetchall():
        print("  ", row)


# 1. A domain check on an empty value: SQL's NOT IN gives NULL, not a failure.
show("Domain, plain SQL: activity NOT IN (...)",
     "SELECT id, activity, activity NOT IN ('EMPLEADO','PENSIONADO') AS fails FROM t ORDER BY id")
show("Domain, compiled: empty values skipped by default (the required pattern counts them)",
     """SELECT id, activity,
       CASE WHEN nullif(trim(activity), '') IS NULL THEN NULL
            ELSE activity NOT IN ('EMPLEADO','PENSIONADO') END AS fails
     FROM t ORDER BY id""")

# 2. Empty means NULL, '' or only spaces, and trim() keeps the no-break space.
show("Empty: trim() does not remove U+00A0",
     "SELECT id, city, length(city), length(trim(city)) FROM t WHERE id = 2")
show("Empty and text normalization, compiled: no-break spaces become spaces before trim",
     """SELECT id, employer,
       nullif(trim(replace(employer, chr(160), ' ')), '') IS NULL AS is_empty,
       length(trim(replace(city, chr(160), ' '))) AS city_len
     FROM t ORDER BY id""")

# 3. Regular expressions: partial in regexp_matches (and Trino's regexp_like).
show("Format, plain: regexp_matches is a partial match",
     r"SELECT id, mobile, regexp_matches(mobile, '3\d{9}') AS partial, regexp_full_match(mobile, '3\d{9}') AS full FROM t ORDER BY id")
print("   -> the compiler always anchors: '^(?:' || pattern || ')$', the same on DuckDB and Trino")

# 4. Numbers read as text keep '.0': a length or format rule on them is wrong.
show("Numbers as text",
     "SELECT id, amount, CAST(amount AS VARCHAR) AS plain FROM t ORDER BY id")
show("Numbers as text, compiled: whole numbers lose the decimals",
     """SELECT id, CASE WHEN amount = trunc(amount) THEN CAST(CAST(amount AS BIGINT) AS VARCHAR)
                    ELSE CAST(amount AS VARCHAR) END AS as_text FROM t ORDER BY id""")

# 5. Placeholders count as empty for 'required', and are their own finding.
show("Required with placeholders: 'ACTUALIZAR' is empty for the required pattern",
     """SELECT id, employer,
       CASE WHEN activity <> 'EMPLEADO' OR activity IS NULL THEN NULL
            ELSE nullif(trim(replace(employer, chr(160), ' ')), '') IS NULL
                 OR upper(trim(employer)) IN ('ACTUALIZAR') END AS fails
     FROM t ORDER BY id""")

# 6. Uniqueness in the same pass, with a normalized key and placeholders out.
show("Unique, compiled: a window over the normalized key, in the same pass as the others",
     """WITH k AS (
       SELECT id, CASE WHEN lower(trim(email)) LIKE 'notiene%' THEN NULL ELSE lower(trim(email)) END AS key FROM t)
     SELECT id, key, CASE WHEN key IS NULL THEN NULL
                          ELSE count(*) OVER (PARTITION BY key) > 1 END AS fails
     FROM k ORDER BY id""")

# 7. The three-valued result of several rules in one pass.
show("One pass, three values per rule: NULL does not apply, TRUE fails, FALSE passes",
     r"""SELECT id,
       CASE WHEN client_type <> 'PN' THEN NULL
            WHEN activity = 'EMPLEADO' THEN nullif(trim(employer), '') IS NULL END AS r_employer,
       CASE WHEN nullif(trim(mobile), '') IS NULL THEN NULL
            ELSE NOT regexp_full_match(mobile, '^(?:3\d{9})$') END AS r_mobile,
       CASE WHEN nullif(trim(city), '') IS NULL THEN NULL
            ELSE regexp_full_match(city, '^[a-zñ].*') END AS r_lowercase
     FROM t ORDER BY id""")
