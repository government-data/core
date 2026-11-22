-- contract: v1.1.2 no longer uses it
-- Licenses are removed (decision 0074): no version from v1.1.2 on reads or
-- writes the license's row. Its policy goes with the table.
DROP TABLE IF EXISTS public.orq_license;
