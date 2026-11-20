-- How the OIDC provider treats each application (decision 0030): the
-- algorithm of its tokens (ES256 for relying parties that only verify RSA or
-- EC keys, such as Trino), PKCE made optional for a confidential client, the
-- IAM policy an account needs to get a code, and machine tokens
-- (`client_credentials`) for its audiences, optionally as a service account.
ALTER TABLE public.oidc_clients
    ADD COLUMN token_alg text NOT NULL DEFAULT 'EdDSA',
    ADD COLUMN pkce_required boolean NOT NULL DEFAULT true,
    ADD COLUMN required_policy text,
    ADD COLUMN grant_client_credentials boolean NOT NULL DEFAULT false,
    ADD COLUMN audiences text[] NOT NULL DEFAULT '{}',
    ADD COLUMN service_account_id uuid REFERENCES public.service_accounts(id) ON DELETE SET NULL;

ALTER TABLE public.oidc_clients
    ADD CONSTRAINT oidc_clients_token_alg CHECK (token_alg IN ('EdDSA', 'ES256')),
    -- A public client always requires PKCE; machine tokens need a secret.
    ADD CONSTRAINT oidc_clients_pkce_public CHECK (pkce_required OR secret_hash IS NOT NULL),
    ADD CONSTRAINT oidc_clients_machine_secret
        CHECK (NOT grant_client_credentials OR secret_hash IS NOT NULL);
