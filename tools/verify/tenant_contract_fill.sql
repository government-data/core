-- Synthetic rows for tools/verify/tenant_contract.sh: up to two rows in every
-- table of the schemas `public` and `quality`, generated from the catalog,
-- so that the check does not depend on a list of tables kept by hand.
--
-- A column with a default takes it (`tenant_id` takes current_tenant(), the
-- main tenant); a nullable column without one is left NULL (but the few a
-- CHECK needs, in `wanted`); a required one
-- gets a value of its type, or a row of the table it references, or the
-- first literal its CHECK names. Tables are filled in passes until a pass
-- fills nothing, so referenced tables come first. Nothing here is real data.
CREATE TEMP TABLE fill_errors (tbl text PRIMARY KEY, error text);

CREATE OR REPLACE FUNCTION pg_temp.synthetic(tbl regclass, col name, typ oid, n int)
RETURNS text LANGUAGE plpgsql AS $$
DECLARE
  ref record;
  lit text;
BEGIN
  -- Columns whose CHECK is a pattern the generic values do not match.
  CASE (SELECT relname FROM pg_class WHERE oid = tbl) || '.' || col
    WHEN 'lake_root_keys.lake' THEN RETURN '''pool:synthetic' || n || '''';
    WHEN 'lake_admin_keys.lake' THEN RETURN '''pool:synthetic' || n || '''';
    WHEN 'lake_replication.warehouse' THEN RETURN '''qd-tenant-synthetic' || n || '''';
    WHEN 'machine_alerts.qd_alert' THEN RETURN '''synthetic''';
    WHEN 'lake_pools.nodes' THEN RETURN '''{n1,n2,n3,n4}''::text[]';
    ELSE NULL;
  END CASE;
  -- A foreign key: a value that the referenced table holds.
  SELECT c.confrelid::regclass AS rel, a2.attname AS refcol INTO ref
  FROM pg_constraint c
  JOIN LATERAL unnest(c.conkey, c.confkey) AS k(src, dst) ON true
  JOIN pg_attribute a1 ON a1.attrelid = c.conrelid AND a1.attnum = k.src
  JOIN pg_attribute a2 ON a2.attrelid = c.confrelid AND a2.attnum = k.dst
  WHERE c.contype = 'f' AND c.conrelid = tbl AND a1.attname = col
  LIMIT 1;
  IF FOUND THEN
    RETURN format('(SELECT %I FROM %s ORDER BY 1 LIMIT 1)', ref.refcol, ref.rel);
  END IF;
  -- A CHECK that lists the allowed values (= ANY, IN): the first one.
  SELECT substring(pg_get_constraintdef(c.oid) FROM '''([^'']*)''') INTO lit
  FROM pg_constraint c
  JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = ANY (c.conkey)
  WHERE c.contype = 'c' AND c.conrelid = tbl AND a.attname = col
    AND pg_get_constraintdef(c.oid) ~ '(= ANY|IN) \(\(?ARRAY\[''|IN \('''
  LIMIT 1;
  IF lit IS NOT NULL THEN
    RETURN format('%L::%s', lit, format_type(typ, NULL));
  END IF;
  RETURN CASE format_type(typ, NULL)
    WHEN 'uuid' THEN 'gen_random_uuid()'
    WHEN 'text' THEN format('%L', 'synthetic' || n)
    WHEN 'character varying' THEN format('%L', 'synthetic' || n)
    WHEN 'citext' THEN format('%L', 'synthetic' || n || '@example.invalid')
    WHEN 'integer' THEN n::text
    WHEN 'bigint' THEN n::text
    WHEN 'smallint' THEN n::text
    WHEN 'numeric' THEN n::text
    WHEN 'double precision' THEN n::text
    WHEN 'real' THEN n::text
    WHEN 'boolean' THEN 'false'
    WHEN 'jsonb' THEN '''{}''::jsonb'
    WHEN 'json' THEN '''{}''::json'
    WHEN 'bytea' THEN format('%L::bytea', '\x' || lpad(to_hex(n), 2, '0'))
    WHEN 'date' THEN format('%L::date', date '2026-01-01' + n)
    WHEN 'timestamp with time zone' THEN format('%L::timestamptz', timestamptz '2026-01-01' + n * interval '1 hour')
    WHEN 'timestamp without time zone' THEN format('%L::timestamp', timestamp '2026-01-01' + n * interval '1 hour')
    WHEN 'interval' THEN '''1 hour''::interval'
    WHEN 'inet' THEN '''192.0.2.1''::inet'
    WHEN 'text[]' THEN '''{}''::text[]'
    WHEN 'uuid[]' THEN '''{}''::uuid[]'
    ELSE format('NULL::%s', format_type(typ, NULL))
  END;
END $$;

DO $$
DECLARE
  t record;
  cols text;
  vals text;
  filled int;
  pass int := 0;
  n int;
  found_rows boolean;
  -- Nullable columns that a table's CHECK needs set.
  wanted text[] := ARRAY['machine_alerts.machine_id', 'quality.jobs.dataset', 'quality.schedules.dataset'];
BEGIN
  LOOP
    pass := pass + 1;
    filled := 0;
    FOR t IN
      SELECT c.oid::regclass AS tbl
      FROM pg_class c JOIN pg_namespace s ON s.oid = c.relnamespace
      WHERE c.relkind IN ('r', 'p') AND s.nspname IN ('public', 'quality')
        AND c.relname NOT IN ('_sqlx_migrations', '_module_migrations')
        AND NOT c.relispartition
      ORDER BY 1::text
    LOOP
      CONTINUE WHEN EXISTS (SELECT 1 FROM fill_errors e WHERE e.tbl = t.tbl::text AND e.error = 'filled');
      -- A table the migrations already filled (a single row, a seed) stays as is.
      EXECUTE format('SELECT EXISTS (SELECT 1 FROM %s)', t.tbl) INTO found_rows;
      IF found_rows THEN
        INSERT INTO fill_errors VALUES (t.tbl::text, 'filled')
          ON CONFLICT (tbl) DO UPDATE SET error = 'filled';
        CONTINUE;
      END IF;
      FOR n IN 1..2 LOOP
        SELECT string_agg(quote_ident(a.attname), ', ' ORDER BY a.attnum),
               string_agg(pg_temp.synthetic(t.tbl, a.attname, a.atttypid, n), ', ' ORDER BY a.attnum)
        INTO cols, vals
        FROM pg_attribute a
        WHERE a.attrelid = t.tbl AND a.attnum > 0 AND NOT a.attisdropped
          AND a.atthasdef IS FALSE AND a.attidentity = '' AND a.attgenerated = ''
          AND (a.attnotnull OR t.tbl::text || '.' || a.attname = ANY (wanted));
        BEGIN
          IF cols IS NULL THEN
            EXECUTE format('INSERT INTO %s DEFAULT VALUES', t.tbl);
          ELSE
            EXECUTE format('INSERT INTO %s (%s) VALUES (%s)', t.tbl, cols, vals);
          END IF;
          IF n = 1 THEN filled := filled + 1; END IF;
          INSERT INTO fill_errors VALUES (t.tbl::text, 'filled')
            ON CONFLICT (tbl) DO UPDATE SET error = 'filled';
        EXCEPTION WHEN OTHERS THEN
          IF n = 1 THEN
            INSERT INTO fill_errors VALUES (t.tbl::text, SQLERRM)
              ON CONFLICT (tbl) DO UPDATE SET error = SQLERRM;
          END IF;
          EXIT;
        END;
      END LOOP;
    END LOOP;
    EXIT WHEN filled = 0 OR pass > 20;
  END LOOP;
END $$;

-- What could not be filled, one line each: the table and the error.
SELECT tbl || ': ' || error FROM fill_errors WHERE error <> 'filled' ORDER BY tbl;
