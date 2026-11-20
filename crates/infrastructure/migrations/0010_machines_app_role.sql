-- The application VM is a machine of role `app` (decision 0066, point 15):
-- its agent keeps the certificate PostgreSQL, Traefik's internal entry point
-- and Dolt serve. It is created by the installer with its first certificate
-- signed on the spot (point 16), so it never joins and gets no OIDC client:
-- `join_tokens.role` and `oidc_clients.machine_role` keep their five roles.
ALTER TABLE public.machines
    DROP CONSTRAINT machines_role_check,
    ADD CONSTRAINT machines_role_check
        CHECK (role = ANY (ARRAY['worker'::text, 'trino-coordinator'::text, 'trino-worker'::text, 'lake'::text, 'lake-replica'::text, 'app'::text]));
