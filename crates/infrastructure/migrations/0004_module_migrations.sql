-- The feature modules' migrations (contracts::Module), applied by
-- infrastructure::module_migrations at startup, one transaction each. A
-- bookkeeping table like sqlx's own `_sqlx_migrations`: no tenant's data.
CREATE TABLE public._module_migrations (
    id text PRIMARY KEY,
    checksum text NOT NULL,
    applied_at timestamp with time zone DEFAULT now() NOT NULL
);
