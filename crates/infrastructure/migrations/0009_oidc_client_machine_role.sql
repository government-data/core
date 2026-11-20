-- A machine client authenticates with the certificate of a machine of one
-- role, not with a secret (decision 0066, point 6; RFC 8705
-- `tls_client_auth`): `machine_role` is that role, the certificate's `OU`.
-- Such a client has no secret and no redirect URI, and is for
-- `client_credentials` only.
ALTER TABLE public.oidc_clients
    ADD COLUMN machine_role text CHECK (machine_role = ANY (ARRAY['worker'::text, 'trino-coordinator'::text, 'trino-worker'::text, 'lake'::text, 'lake-replica'::text]));

-- Machine tokens need a secret or, now, a machine role.
ALTER TABLE public.oidc_clients
    DROP CONSTRAINT oidc_clients_machine_secret,
    ADD CONSTRAINT oidc_clients_machine_secret
        CHECK (NOT grant_client_credentials OR secret_hash IS NOT NULL OR machine_role IS NOT NULL);

-- A secret and a machine role never together; a machine role's client gets
-- machine tokens and nothing else.
ALTER TABLE public.oidc_clients
    ADD CONSTRAINT oidc_clients_machine_role_only CHECK (
        machine_role IS NULL
        OR (secret_hash IS NULL AND grant_client_credentials AND cardinality(redirect_uris) = 0)
    );
