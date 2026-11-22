-- contract: v1.1.2 no longer uses it
-- Payments are removed (decision 0075): no version from v1.1.2 on reads or
-- writes the `payments` schema. Its three tables, their sequences and
-- policies go with it.
DROP SCHEMA IF EXISTS payments CASCADE;
