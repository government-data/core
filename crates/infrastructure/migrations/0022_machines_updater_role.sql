-- The installation's update agent is a machine of role `updater` (decision
-- 0003): it calls `/updater/v1/` on the internal entry point with its
-- certificate. As `app`, it is made by the installer with its certificate
-- signed on the spot, so it never joins and gets no OIDC client:
-- `join_tokens.role` and `oidc_clients.machine_role` keep their five roles.
ALTER TABLE public.machines
    DROP CONSTRAINT machines_role_check,
    ADD CONSTRAINT machines_role_check
        CHECK (role = ANY (ARRAY['worker'::text, 'trino-coordinator'::text, 'trino-worker'::text, 'lake'::text, 'lake-replica'::text, 'app'::text, 'updater'::text]));
