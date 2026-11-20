-- The alerts Grafana delivers about machines (decision 0072, points 11 and
-- 12, step 5 as amended on 2026-11-16): each active alert once, deleted
-- when it resolves, never a history. A machine's state is computed from
-- its rows (the worst severity), not stored.
--
-- A machine's row is keyed by Grafana's fingerprint. A row with no machine
-- is keyed by Grafana's `alertname` and the rule's name instead: a
-- `DatasourceError` or `DatasourceNoData` (metrics not available), or the
-- heartbeat, whose row is `alerting_heartbeat` under both and whose
-- `received_at` is its last delivery.
--
-- `values` holds the alert's numbers by query (`{"A": 0.07, "C": 1}`) and
-- the labels the texts name (`path`, `device`): never Grafana's prose.

CREATE TABLE public.machine_alerts (
    id uuid DEFAULT gen_random_uuid() PRIMARY KEY,
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL REFERENCES public.tenants(id) ON DELETE RESTRICT,
    machine_id uuid REFERENCES public.machines(id) ON DELETE CASCADE,
    -- Null on a machine's row.
    alertname text CHECK (alertname ~ '^[A-Za-z0-9_]{1,64}$'),
    rule_name text CHECK (rule_name ~ '^[A-Za-z0-9_.-]{1,128}$'),
    fingerprint text NOT NULL CHECK (fingerprint ~ '^[0-9A-Za-z]{1,64}$'),
    -- The key of the text (`machines.alerts.<qd_alert>`).
    qd_alert text NOT NULL CHECK (qd_alert ~ '^[a-z_]{1,64}$'),
    -- None for the heartbeat.
    severity text CHECK (severity = ANY (ARRAY['warning'::text, 'critical'::text])),
    starts_at timestamp with time zone NOT NULL,
    received_at timestamp with time zone DEFAULT now() NOT NULL,
    "values" jsonb DEFAULT '{}'::jsonb NOT NULL CHECK (jsonb_typeof("values") = 'object'),
    CONSTRAINT machine_alerts_key CHECK (
        (machine_id IS NOT NULL AND alertname IS NULL AND rule_name IS NULL)
        OR (machine_id IS NULL AND alertname IS NOT NULL AND rule_name IS NOT NULL)
    ),
    CONSTRAINT machine_alerts_fingerprint UNIQUE NULLS NOT DISTINCT (tenant_id, machine_id, fingerprint)
);

-- The rows with no machine, by what Grafana names them.
CREATE UNIQUE INDEX machine_alerts_machineless ON public.machine_alerts USING btree (tenant_id, alertname, rule_name) WHERE (machine_id IS NULL);

ALTER TABLE public.machine_alerts ENABLE ROW LEVEL SECURITY;
ALTER TABLE ONLY public.machine_alerts FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON public.machine_alerts USING ((tenant_id = public.current_tenant())) WITH CHECK ((tenant_id = public.current_tenant()));
