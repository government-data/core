-- The schema of the quality service, squashed from the orchestrator's 94
-- migrations without the KNIME engine (servers, agents, workflows,
-- deployments, campaigns, test runs, metrics, contingency) and without the
-- flows, areas, executions and Python engine that followed it, and with the
-- IAM permissions that replace the global roles (ADR 0016).
-- Tenants are isolated with row-level security on `tenant_id` (see
-- `current_tenant()`).

-- pg_dump creates the functions before the tables they read.
SET LOCAL check_function_bodies = false;



-- Name: payments; Type: SCHEMA; Schema: -; Owner: -

CREATE SCHEMA payments;

-- Name: pgcrypto; Type: EXTENSION; Schema: -; Owner: -

CREATE EXTENSION IF NOT EXISTS pgcrypto WITH SCHEMA public;

-- Name: tenants_with_email(text, text); Type: FUNCTION; Schema: public; Owner: -

CREATE FUNCTION public.tenants_with_email(wanted_email text, wanted_cell text) RETURNS SETOF uuid
    LANGUAGE plpgsql
    AS $$
DECLARE
    tenant UUID;
BEGIN
    FOR tenant IN
        SELECT id FROM tenants WHERE cell = wanted_cell AND status <> 'closed' ORDER BY slug
    LOOP
        PERFORM set_config('app.tenant_id', tenant::text, true);
        IF EXISTS (
            SELECT 1 FROM admin_users WHERE email = wanted_email AND password_hash <> '!pending'
        ) THEN
            RETURN NEXT tenant;
        END IF;
    END LOOP;
    PERFORM set_config('app.tenant_id', '00000000-0000-0000-0000-000000000000', true);
END
$$;

-- Name: notify_permissions(); Type: FUNCTION; Schema: public; Owner: -

CREATE FUNCTION public.notify_permissions() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        PERFORM pg_notify('permissions', OLD.tenant_id::text);
    ELSE
        PERFORM pg_notify('permissions', NEW.tenant_id::text);
    END IF;
    RETURN NULL;
END;
$$;

-- Name: current_tenant(); Type: FUNCTION; Schema: public; Owner: -

CREATE FUNCTION public.current_tenant() RETURNS uuid
    LANGUAGE sql STABLE PARALLEL SAFE
    AS $$
    SELECT COALESCE(
        NULLIF(current_setting('app.tenant_id', true), '')::uuid,
        '00000000-0000-0000-0000-000000000001'::uuid
    )
$$;



-- Name: payment_status_history; Type: TABLE; Schema: payments; Owner: -

CREATE TABLE payments.payment_status_history (
    id bigint NOT NULL,
    payment_id uuid NOT NULL,
    from_status text,
    to_status text NOT NULL,
    reason text,
    occurred_at timestamp with time zone NOT NULL,
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL
);

ALTER TABLE ONLY payments.payment_status_history FORCE ROW LEVEL SECURITY;

-- Name: payment_status_history_id_seq; Type: SEQUENCE; Schema: payments; Owner: -

ALTER TABLE payments.payment_status_history ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME payments.payment_status_history_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

-- Name: payments; Type: TABLE; Schema: payments; Owner: -

CREATE TABLE payments.payments (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    provider_reference text,
    amount_minor_units bigint NOT NULL,
    currency text NOT NULL,
    status text NOT NULL,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL,
    CONSTRAINT payments_amount_minor_units_check CHECK ((amount_minor_units >= 0))
);

ALTER TABLE ONLY payments.payments FORCE ROW LEVEL SECURITY;

-- Name: webhook_events; Type: TABLE; Schema: payments; Owner: -

CREATE TABLE payments.webhook_events (
    id bigint NOT NULL,
    event_id text,
    payload bytea NOT NULL,
    signature text,
    accepted boolean NOT NULL,
    reason text,
    received_at timestamp with time zone DEFAULT now() NOT NULL
);

-- Name: webhook_events_id_seq; Type: SEQUENCE; Schema: payments; Owner: -

ALTER TABLE payments.webhook_events ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME payments.webhook_events_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

-- Name: admin_audit_events; Type: TABLE; Schema: public; Owner: -

CREATE TABLE public.admin_audit_events (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    event_type text NOT NULL,
    admin_id uuid,
    email_attempted text,
    ip text NOT NULL,
    user_agent text,
    occurred_at timestamp with time zone NOT NULL,
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL
);

ALTER TABLE ONLY public.admin_audit_events FORCE ROW LEVEL SECURITY;

-- Name: admin_ip_lockouts; Type: TABLE; Schema: public; Owner: -

CREATE TABLE public.admin_ip_lockouts (
    ip text NOT NULL,
    failed_attempts integer DEFAULT 0 NOT NULL,
    locked_until timestamp with time zone,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

-- Name: admin_oauth_identities; Type: TABLE; Schema: public; Owner: -

CREATE TABLE public.admin_oauth_identities (
    provider text NOT NULL,
    subject text NOT NULL,
    admin_id uuid NOT NULL,
    email text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL
);

ALTER TABLE ONLY public.admin_oauth_identities FORCE ROW LEVEL SECURITY;

-- Name: admin_password_resets; Type: TABLE; Schema: public; Owner: -

CREATE TABLE public.admin_password_resets (
    token_hash text NOT NULL,
    admin_id uuid NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL
);

ALTER TABLE ONLY public.admin_password_resets FORCE ROW LEVEL SECURITY;

-- Name: admin_sessions; Type: TABLE; Schema: public; Owner: -

CREATE TABLE public.admin_sessions (
    token text NOT NULL,
    admin_id uuid NOT NULL,
    csrf_token text NOT NULL,
    created_at timestamp with time zone NOT NULL,
    last_seen_at timestamp with time zone NOT NULL,
    absolute_expires_at timestamp with time zone NOT NULL,
    role text DEFAULT 'admin'::text NOT NULL,
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL
);

ALTER TABLE ONLY public.admin_sessions FORCE ROW LEVEL SECURITY;

-- Name: admin_users; Type: TABLE; Schema: public; Owner: -

CREATE TABLE public.admin_users (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    email text NOT NULL,
    password_hash text NOT NULL,
    failed_attempts integer DEFAULT 0 NOT NULL,
    locked_until timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    role text DEFAULT 'admin'::text NOT NULL,
    display_name text,
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL,
    locale text,
    avatar bytea,
    avatar_type text,
    avatar_updated_at timestamp with time zone,
    avatar_own boolean DEFAULT false NOT NULL,
    CONSTRAINT admin_users_locale_check CHECK ((locale = ANY (ARRAY['es'::text, 'en'::text])))
);

ALTER TABLE ONLY public.admin_users FORCE ROW LEVEL SECURITY;

-- Name: login_handoffs; Type: TABLE; Schema: public; Owner: -

CREATE TABLE public.login_handoffs (
    token_hash text NOT NULL,
    admin_id uuid NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL
);

ALTER TABLE ONLY public.login_handoffs FORCE ROW LEVEL SECURITY;

-- Name: oauth_pending_authorizations; Type: TABLE; Schema: public; Owner: -

CREATE TABLE public.oauth_pending_authorizations (
    state text NOT NULL,
    provider text NOT NULL,
    nonce text NOT NULL,
    code_verifier text NOT NULL,
    redirect_uri text NOT NULL,
    created_at timestamp with time zone NOT NULL,
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL,
    link_admin_id uuid
);

ALTER TABLE ONLY public.oauth_pending_authorizations FORCE ROW LEVEL SECURITY;

-- Name: oidc_authorization_codes; Type: TABLE; Schema: public; Owner: -

CREATE TABLE public.oidc_authorization_codes (
    code_hash text NOT NULL,
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL,
    client_id text NOT NULL,
    admin_id uuid NOT NULL,
    redirect_uri text NOT NULL,
    code_challenge text NOT NULL,
    nonce text,
    scopes text[] NOT NULL,
    auth_time timestamp with time zone NOT NULL,
    expires_at timestamp with time zone NOT NULL
);

ALTER TABLE ONLY public.oidc_authorization_codes FORCE ROW LEVEL SECURITY;

-- Name: oidc_clients; Type: TABLE; Schema: public; Owner: -

CREATE TABLE public.oidc_clients (
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL,
    client_id text NOT NULL,
    name text NOT NULL,
    redirect_uris text[] NOT NULL,
    secret_hash text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    -- Where it starts and what it is for: with them it is listed in the
    -- avatar's «Aplicaciones».
    home_url text,
    detail text DEFAULT ''::text NOT NULL
);

ALTER TABLE ONLY public.oidc_clients FORCE ROW LEVEL SECURITY;

-- Name: orq_license; Type: TABLE; Schema: public; Owner: -

CREATE TABLE public.orq_license (
    key_sealed text,
    permit text,
    fetched_at timestamp with time zone,
    last_attempt_at timestamp with time zone,
    last_error text,
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL,
    reminders jsonb DEFAULT '[]'::jsonb NOT NULL
);

ALTER TABLE ONLY public.orq_license FORCE ROW LEVEL SECURITY;

-- Name: orq_maintenance; Type: TABLE; Schema: public; Owner: -

CREATE TABLE public.orq_maintenance (
    id boolean DEFAULT true NOT NULL,
    reason text NOT NULL,
    started_at timestamp with time zone DEFAULT now() NOT NULL,
    until timestamp with time zone NOT NULL,
    CONSTRAINT orq_maintenance_id_check CHECK (id)
);

-- Name: orq_update_history; Type: TABLE; Schema: public; Owner: -

CREATE TABLE public.orq_update_history (
    id bigint NOT NULL,
    kind text NOT NULL,
    version text NOT NULL,
    from_version text NOT NULL,
    detail text NOT NULL,
    at timestamp with time zone NOT NULL
);

-- Name: orq_update_history_id_seq; Type: SEQUENCE; Schema: public; Owner: -

CREATE SEQUENCE public.orq_update_history_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

-- Name: orq_update_history_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -

ALTER SEQUENCE public.orq_update_history_id_seq OWNED BY public.orq_update_history.id;

-- Name: orq_updater; Type: TABLE; Schema: public; Owner: -

CREATE TABLE public.orq_updater (
    id boolean DEFAULT true NOT NULL,
    report jsonb,
    reported_at timestamp with time zone,
    policy jsonb,
    policy_updated_at timestamp with time zone,
    policy_updated_by uuid,
    request_id uuid,
    requested_at timestamp with time zone,
    requested_by uuid,
    notified_available text,
    notified_result text,
    CONSTRAINT orq_updater_id_check CHECK (id)
);

-- Name: secrets; Type: TABLE; Schema: public; Owner: -

CREATE TABLE public.secrets (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    name text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    value text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL,
    kind text DEFAULT 'value'::text NOT NULL,
    meta jsonb DEFAULT '{}'::jsonb NOT NULL
);

ALTER TABLE ONLY public.secrets FORCE ROW LEVEL SECURITY;

-- Name: system_settings; Type: TABLE; Schema: public; Owner: -

CREATE TABLE public.system_settings (
    section text NOT NULL,
    data jsonb NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_by uuid,
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL,
    CONSTRAINT system_settings_section_check CHECK ((section = ANY (ARRAY['ldap'::text, 'mail'::text, 'oauth'::text])))
);

ALTER TABLE ONLY public.system_settings FORCE ROW LEVEL SECURITY;

-- Name: tenants; Type: TABLE; Schema: public; Owner: -

CREATE TABLE public.tenants (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    slug text NOT NULL,
    name text NOT NULL,
    status text DEFAULT 'active'::text NOT NULL,
    cell text DEFAULT 'main'::text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT tenants_status_check CHECK ((status = ANY (ARRAY['active'::text, 'suspended'::text, 'closed'::text]))),
    CONSTRAINT tenants_slug_check CHECK ((slug ~ '^[a-z0-9]([a-z0-9-]{0,38}[a-z0-9])?$'::text))
);

-- Name: orq_update_history id; Type: DEFAULT; Schema: public; Owner: -

ALTER TABLE ONLY public.orq_update_history ALTER COLUMN id SET DEFAULT nextval('public.orq_update_history_id_seq'::regclass);

-- Name: payment_status_history payment_status_history_pkey; Type: CONSTRAINT; Schema: payments; Owner: -

ALTER TABLE ONLY payments.payment_status_history
    ADD CONSTRAINT payment_status_history_pkey PRIMARY KEY (id);

-- Name: payments payments_pkey; Type: CONSTRAINT; Schema: payments; Owner: -

ALTER TABLE ONLY payments.payments
    ADD CONSTRAINT payments_pkey PRIMARY KEY (id);

-- Name: webhook_events webhook_events_pkey; Type: CONSTRAINT; Schema: payments; Owner: -

ALTER TABLE ONLY payments.webhook_events
    ADD CONSTRAINT webhook_events_pkey PRIMARY KEY (id);

-- Name: admin_audit_events admin_audit_events_pkey; Type: CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.admin_audit_events
    ADD CONSTRAINT admin_audit_events_pkey PRIMARY KEY (id);

-- Name: admin_ip_lockouts admin_ip_lockouts_pkey; Type: CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.admin_ip_lockouts
    ADD CONSTRAINT admin_ip_lockouts_pkey PRIMARY KEY (ip);

-- Name: admin_oauth_identities admin_oauth_identities_pkey; Type: CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.admin_oauth_identities
    ADD CONSTRAINT admin_oauth_identities_pkey PRIMARY KEY (tenant_id, provider, subject);

-- Name: admin_password_resets admin_password_resets_pkey; Type: CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.admin_password_resets
    ADD CONSTRAINT admin_password_resets_pkey PRIMARY KEY (token_hash);

-- Name: admin_sessions admin_sessions_pkey; Type: CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.admin_sessions
    ADD CONSTRAINT admin_sessions_pkey PRIMARY KEY (token);

-- Name: admin_users admin_users_pkey; Type: CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.admin_users
    ADD CONSTRAINT admin_users_pkey PRIMARY KEY (id);

-- Name: admin_users admin_users_tenant_email_key; Type: CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.admin_users
    ADD CONSTRAINT admin_users_tenant_email_key UNIQUE (tenant_id, email);

-- Name: secrets secrets_pkey; Type: CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.secrets
    ADD CONSTRAINT secrets_pkey PRIMARY KEY (id);

-- Name: secrets secrets_tenant_id_name_key; Type: CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.secrets
    ADD CONSTRAINT secrets_tenant_id_name_key UNIQUE (tenant_id, name);

-- Name: login_handoffs login_handoffs_pkey; Type: CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.login_handoffs
    ADD CONSTRAINT login_handoffs_pkey PRIMARY KEY (token_hash);

-- Name: oauth_pending_authorizations oauth_pending_authorizations_pkey; Type: CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.oauth_pending_authorizations
    ADD CONSTRAINT oauth_pending_authorizations_pkey PRIMARY KEY (state);

-- Name: oidc_authorization_codes oidc_authorization_codes_pkey; Type: CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.oidc_authorization_codes
    ADD CONSTRAINT oidc_authorization_codes_pkey PRIMARY KEY (code_hash);

-- Name: oidc_clients oidc_clients_pkey; Type: CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.oidc_clients
    ADD CONSTRAINT oidc_clients_pkey PRIMARY KEY (tenant_id, client_id);

-- Name: orq_license orq_license_pkey; Type: CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.orq_license
    ADD CONSTRAINT orq_license_pkey PRIMARY KEY (tenant_id);

-- Name: orq_maintenance orq_maintenance_pkey; Type: CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.orq_maintenance
    ADD CONSTRAINT orq_maintenance_pkey PRIMARY KEY (id);

-- Name: orq_update_history orq_update_history_at_version_kind_key; Type: CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.orq_update_history
    ADD CONSTRAINT orq_update_history_at_version_kind_key UNIQUE (at, version, kind);

-- Name: orq_update_history orq_update_history_pkey; Type: CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.orq_update_history
    ADD CONSTRAINT orq_update_history_pkey PRIMARY KEY (id);

-- Name: orq_updater orq_updater_pkey; Type: CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.orq_updater
    ADD CONSTRAINT orq_updater_pkey PRIMARY KEY (id);

-- Name: system_settings system_settings_pkey; Type: CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.system_settings
    ADD CONSTRAINT system_settings_pkey PRIMARY KEY (tenant_id, section);

-- Name: tenants tenants_pkey; Type: CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.tenants
    ADD CONSTRAINT tenants_pkey PRIMARY KEY (id);

-- Name: tenants tenants_slug_key; Type: CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.tenants
    ADD CONSTRAINT tenants_slug_key UNIQUE (slug);

-- Name: tenants tenants_slug_reserved; Type: CHECK CONSTRAINT; Schema: public; Owner: -

ALTER TABLE public.tenants
    ADD CONSTRAINT tenants_slug_reserved CHECK ((slug <> ALL (ARRAY['entrar'::text, 'www'::text, 'api'::text, 'app'::text, 'admin'::text, 'gestion'::text, 'licencias'::text, 'correo'::text, 'mail'::text]))) NOT VALID;

-- Name: payment_status_history_payment_id_idx; Type: INDEX; Schema: payments; Owner: -

CREATE INDEX payment_status_history_payment_id_idx ON payments.payment_status_history USING btree (payment_id);

-- Name: payments_provider_reference_idx; Type: INDEX; Schema: payments; Owner: -

CREATE UNIQUE INDEX payments_provider_reference_idx ON payments.payments USING btree (provider_reference) WHERE (provider_reference IS NOT NULL);

-- Name: payments_webhook_events_event_id_idx; Type: INDEX; Schema: payments; Owner: -

CREATE UNIQUE INDEX payments_webhook_events_event_id_idx ON payments.webhook_events USING btree (event_id) WHERE (event_id IS NOT NULL);

-- Name: admin_audit_events_admin_id_idx; Type: INDEX; Schema: public; Owner: -

CREATE INDEX admin_audit_events_admin_id_idx ON public.admin_audit_events USING btree (admin_id);

-- Name: admin_audit_events_occurred_at_idx; Type: INDEX; Schema: public; Owner: -

CREATE INDEX admin_audit_events_occurred_at_idx ON public.admin_audit_events USING btree (occurred_at DESC);

-- Name: admin_audit_events_tenant_idx; Type: INDEX; Schema: public; Owner: -

CREATE INDEX admin_audit_events_tenant_idx ON public.admin_audit_events USING btree (tenant_id);

-- Name: admin_oauth_identities_admin_id_idx; Type: INDEX; Schema: public; Owner: -

CREATE INDEX admin_oauth_identities_admin_id_idx ON public.admin_oauth_identities USING btree (admin_id);

-- Name: admin_password_resets_admin_id_idx; Type: INDEX; Schema: public; Owner: -

CREATE INDEX admin_password_resets_admin_id_idx ON public.admin_password_resets USING btree (admin_id);

-- Name: admin_password_resets_tenant_idx; Type: INDEX; Schema: public; Owner: -

CREATE INDEX admin_password_resets_tenant_idx ON public.admin_password_resets USING btree (tenant_id);

-- Name: admin_sessions_admin_id_idx; Type: INDEX; Schema: public; Owner: -

CREATE INDEX admin_sessions_admin_id_idx ON public.admin_sessions USING btree (admin_id);

-- Name: secrets_tenant_idx; Type: INDEX; Schema: public; Owner: -

CREATE INDEX secrets_tenant_idx ON public.secrets USING btree (tenant_id);

-- Name: login_handoffs_admin_id_idx; Type: INDEX; Schema: public; Owner: -

CREATE INDEX login_handoffs_admin_id_idx ON public.login_handoffs USING btree (admin_id);

-- Name: login_handoffs_tenant_idx; Type: INDEX; Schema: public; Owner: -

CREATE INDEX login_handoffs_tenant_idx ON public.login_handoffs USING btree (tenant_id);

-- Name: oidc_authorization_codes_expires_idx; Type: INDEX; Schema: public; Owner: -

CREATE INDEX oidc_authorization_codes_expires_idx ON public.oidc_authorization_codes USING btree (expires_at);

-- Name: orq_update_history_at_idx; Type: INDEX; Schema: public; Owner: -

CREATE INDEX orq_update_history_at_idx ON public.orq_update_history USING btree (at DESC);

-- Name: admin_users notify_permissions; Type: TRIGGER; Schema: public; Owner: -

CREATE TRIGGER notify_permissions AFTER DELETE OR UPDATE OF role ON public.admin_users FOR EACH ROW EXECUTE FUNCTION public.notify_permissions();

-- Name: payment_status_history payment_status_history_payment_id_fkey; Type: FK CONSTRAINT; Schema: payments; Owner: -

ALTER TABLE ONLY payments.payment_status_history
    ADD CONSTRAINT payment_status_history_payment_id_fkey FOREIGN KEY (payment_id) REFERENCES payments.payments(id) ON DELETE CASCADE;

-- Name: payment_status_history payment_status_history_tenant_id_fkey; Type: FK CONSTRAINT; Schema: payments; Owner: -

ALTER TABLE ONLY payments.payment_status_history
    ADD CONSTRAINT payment_status_history_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE RESTRICT;

-- Name: payments payments_tenant_id_fkey; Type: FK CONSTRAINT; Schema: payments; Owner: -

ALTER TABLE ONLY payments.payments
    ADD CONSTRAINT payments_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE RESTRICT;

-- Name: admin_audit_events admin_audit_events_tenant_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.admin_audit_events
    ADD CONSTRAINT admin_audit_events_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE RESTRICT;

-- Name: admin_oauth_identities admin_oauth_identities_admin_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.admin_oauth_identities
    ADD CONSTRAINT admin_oauth_identities_admin_id_fkey FOREIGN KEY (admin_id) REFERENCES public.admin_users(id) ON DELETE CASCADE;

-- Name: admin_oauth_identities admin_oauth_identities_tenant_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.admin_oauth_identities
    ADD CONSTRAINT admin_oauth_identities_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE RESTRICT;

-- Name: admin_password_resets admin_password_resets_admin_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.admin_password_resets
    ADD CONSTRAINT admin_password_resets_admin_id_fkey FOREIGN KEY (admin_id) REFERENCES public.admin_users(id) ON DELETE CASCADE;

-- Name: admin_password_resets admin_password_resets_tenant_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.admin_password_resets
    ADD CONSTRAINT admin_password_resets_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE RESTRICT;

-- Name: admin_sessions admin_sessions_admin_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.admin_sessions
    ADD CONSTRAINT admin_sessions_admin_id_fkey FOREIGN KEY (admin_id) REFERENCES public.admin_users(id) ON DELETE CASCADE;

-- Name: admin_sessions admin_sessions_tenant_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.admin_sessions
    ADD CONSTRAINT admin_sessions_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE RESTRICT;

-- Name: admin_users admin_users_tenant_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.admin_users
    ADD CONSTRAINT admin_users_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE RESTRICT;

-- Name: secrets secrets_tenant_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.secrets
    ADD CONSTRAINT secrets_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE RESTRICT;

-- Name: login_handoffs login_handoffs_admin_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.login_handoffs
    ADD CONSTRAINT login_handoffs_admin_id_fkey FOREIGN KEY (admin_id) REFERENCES public.admin_users(id) ON DELETE CASCADE;

-- Name: login_handoffs login_handoffs_tenant_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.login_handoffs
    ADD CONSTRAINT login_handoffs_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE RESTRICT;

-- Name: oauth_pending_authorizations oauth_pending_authorizations_link_admin_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.oauth_pending_authorizations
    ADD CONSTRAINT oauth_pending_authorizations_link_admin_id_fkey FOREIGN KEY (link_admin_id) REFERENCES public.admin_users(id) ON DELETE CASCADE;

-- Name: oauth_pending_authorizations oauth_pending_authorizations_tenant_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.oauth_pending_authorizations
    ADD CONSTRAINT oauth_pending_authorizations_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE RESTRICT;

-- Name: oidc_authorization_codes oidc_authorization_codes_admin_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.oidc_authorization_codes
    ADD CONSTRAINT oidc_authorization_codes_admin_id_fkey FOREIGN KEY (admin_id) REFERENCES public.admin_users(id) ON DELETE CASCADE;

-- Name: oidc_authorization_codes oidc_authorization_codes_tenant_id_client_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.oidc_authorization_codes
    ADD CONSTRAINT oidc_authorization_codes_tenant_id_client_id_fkey FOREIGN KEY (tenant_id, client_id) REFERENCES public.oidc_clients(tenant_id, client_id) ON DELETE CASCADE;

-- Name: oidc_authorization_codes oidc_authorization_codes_tenant_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.oidc_authorization_codes
    ADD CONSTRAINT oidc_authorization_codes_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE RESTRICT;

-- Name: oidc_clients oidc_clients_tenant_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.oidc_clients
    ADD CONSTRAINT oidc_clients_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE RESTRICT;

-- Name: orq_license orq_license_tenant_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.orq_license
    ADD CONSTRAINT orq_license_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE RESTRICT;

-- Name: orq_updater orq_updater_policy_updated_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.orq_updater
    ADD CONSTRAINT orq_updater_policy_updated_by_fkey FOREIGN KEY (policy_updated_by) REFERENCES public.admin_users(id) ON DELETE SET NULL;

-- Name: orq_updater orq_updater_requested_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.orq_updater
    ADD CONSTRAINT orq_updater_requested_by_fkey FOREIGN KEY (requested_by) REFERENCES public.admin_users(id) ON DELETE SET NULL;

-- Name: system_settings system_settings_tenant_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.system_settings
    ADD CONSTRAINT system_settings_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE RESTRICT;

-- Name: system_settings system_settings_updated_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -

ALTER TABLE ONLY public.system_settings
    ADD CONSTRAINT system_settings_updated_by_fkey FOREIGN KEY (updated_by) REFERENCES public.admin_users(id) ON DELETE SET NULL;

-- Name: payment_status_history isolation; Type: POLICY; Schema: payments; Owner: -

CREATE POLICY isolation ON payments.payment_status_history USING ((tenant_id = public.current_tenant())) WITH CHECK ((tenant_id = public.current_tenant()));

-- Name: payments isolation; Type: POLICY; Schema: payments; Owner: -

CREATE POLICY isolation ON payments.payments USING ((tenant_id = public.current_tenant())) WITH CHECK ((tenant_id = public.current_tenant()));

-- Name: payment_status_history; Type: ROW SECURITY; Schema: payments; Owner: -

ALTER TABLE payments.payment_status_history ENABLE ROW LEVEL SECURITY;

-- Name: payments; Type: ROW SECURITY; Schema: payments; Owner: -

ALTER TABLE payments.payments ENABLE ROW LEVEL SECURITY;

-- Name: admin_audit_events; Type: ROW SECURITY; Schema: public; Owner: -

ALTER TABLE public.admin_audit_events ENABLE ROW LEVEL SECURITY;

-- Name: admin_oauth_identities; Type: ROW SECURITY; Schema: public; Owner: -

ALTER TABLE public.admin_oauth_identities ENABLE ROW LEVEL SECURITY;

-- Name: admin_password_resets; Type: ROW SECURITY; Schema: public; Owner: -

ALTER TABLE public.admin_password_resets ENABLE ROW LEVEL SECURITY;

-- Name: admin_sessions; Type: ROW SECURITY; Schema: public; Owner: -

ALTER TABLE public.admin_sessions ENABLE ROW LEVEL SECURITY;

-- Name: admin_users; Type: ROW SECURITY; Schema: public; Owner: -

ALTER TABLE public.admin_users ENABLE ROW LEVEL SECURITY;

-- Name: admin_audit_events isolation; Type: POLICY; Schema: public; Owner: -

CREATE POLICY isolation ON public.admin_audit_events USING ((tenant_id = public.current_tenant())) WITH CHECK ((tenant_id = public.current_tenant()));

-- Name: admin_oauth_identities isolation; Type: POLICY; Schema: public; Owner: -

CREATE POLICY isolation ON public.admin_oauth_identities USING ((tenant_id = public.current_tenant())) WITH CHECK ((tenant_id = public.current_tenant()));

-- Name: admin_password_resets isolation; Type: POLICY; Schema: public; Owner: -

CREATE POLICY isolation ON public.admin_password_resets USING ((tenant_id = public.current_tenant())) WITH CHECK ((tenant_id = public.current_tenant()));

-- Name: admin_sessions isolation; Type: POLICY; Schema: public; Owner: -

CREATE POLICY isolation ON public.admin_sessions USING ((tenant_id = public.current_tenant())) WITH CHECK ((tenant_id = public.current_tenant()));

-- Name: admin_users isolation; Type: POLICY; Schema: public; Owner: -

CREATE POLICY isolation ON public.admin_users USING ((tenant_id = public.current_tenant())) WITH CHECK ((tenant_id = public.current_tenant()));

-- Name: login_handoffs isolation; Type: POLICY; Schema: public; Owner: -

CREATE POLICY isolation ON public.login_handoffs USING ((tenant_id = public.current_tenant())) WITH CHECK ((tenant_id = public.current_tenant()));

-- Name: oauth_pending_authorizations isolation; Type: POLICY; Schema: public; Owner: -

CREATE POLICY isolation ON public.oauth_pending_authorizations USING ((tenant_id = public.current_tenant())) WITH CHECK ((tenant_id = public.current_tenant()));

-- Name: oidc_authorization_codes isolation; Type: POLICY; Schema: public; Owner: -

CREATE POLICY isolation ON public.oidc_authorization_codes USING ((tenant_id = public.current_tenant())) WITH CHECK ((tenant_id = public.current_tenant()));

-- Name: oidc_clients isolation; Type: POLICY; Schema: public; Owner: -

CREATE POLICY isolation ON public.oidc_clients USING ((tenant_id = public.current_tenant())) WITH CHECK ((tenant_id = public.current_tenant()));

-- Name: orq_license isolation; Type: POLICY; Schema: public; Owner: -

CREATE POLICY isolation ON public.orq_license USING ((tenant_id = public.current_tenant())) WITH CHECK ((tenant_id = public.current_tenant()));

-- Name: secrets isolation; Type: POLICY; Schema: public; Owner: -

CREATE POLICY isolation ON public.secrets USING ((tenant_id = public.current_tenant())) WITH CHECK ((tenant_id = public.current_tenant()));

-- Name: system_settings isolation; Type: POLICY; Schema: public; Owner: -

CREATE POLICY isolation ON public.system_settings USING ((tenant_id = public.current_tenant())) WITH CHECK ((tenant_id = public.current_tenant()));

-- Name: login_handoffs; Type: ROW SECURITY; Schema: public; Owner: -

ALTER TABLE public.login_handoffs ENABLE ROW LEVEL SECURITY;

-- Name: oauth_pending_authorizations; Type: ROW SECURITY; Schema: public; Owner: -

ALTER TABLE public.oauth_pending_authorizations ENABLE ROW LEVEL SECURITY;

-- Name: oidc_authorization_codes; Type: ROW SECURITY; Schema: public; Owner: -

ALTER TABLE public.oidc_authorization_codes ENABLE ROW LEVEL SECURITY;

-- Name: oidc_clients; Type: ROW SECURITY; Schema: public; Owner: -

ALTER TABLE public.oidc_clients ENABLE ROW LEVEL SECURITY;

-- Name: orq_license; Type: ROW SECURITY; Schema: public; Owner: -

ALTER TABLE public.orq_license ENABLE ROW LEVEL SECURITY;

-- Name: secrets; Type: ROW SECURITY; Schema: public; Owner: -

ALTER TABLE public.secrets ENABLE ROW LEVEL SECURITY;

-- Name: system_settings; Type: ROW SECURITY; Schema: public; Owner: -

ALTER TABLE public.system_settings ENABLE ROW LEVEL SECURITY;


-- The principal tenant (the installation's own).
INSERT INTO public.tenants VALUES ('00000000-0000-0000-0000-000000000001', 'main', 'Main', 'active', 'main', '2026-10-28 19:57:25.817481+00', '2026-10-28 19:57:25.817481+00');

-- IAM (ADR 0016): groups, custom policies, what is attached to whom, and
-- service accounts. The managed policies live in the code (`iam::managed`);
-- attachments name policies, managed or custom.

CREATE TABLE public.iam_groups (
    id uuid DEFAULT gen_random_uuid() PRIMARY KEY,
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL REFERENCES public.tenants(id) ON DELETE RESTRICT,
    name text NOT NULL CHECK (name ~ '^[a-z][a-z0-9-]{0,63}$'),
    description text DEFAULT ''::text NOT NULL,
    -- The directory groups (DNs, in lower case) whose members belong.
    directory_groups text[] DEFAULT '{}'::text[] NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    UNIQUE (tenant_id, name)
);

CREATE TABLE public.iam_group_members (
    group_id uuid NOT NULL REFERENCES public.iam_groups(id) ON DELETE CASCADE,
    admin_id uuid NOT NULL REFERENCES public.admin_users(id) ON DELETE CASCADE,
    -- `manual`: added in Settings; `directory`: by a directory group, at
    -- every sign-in.
    source text NOT NULL CHECK (source = ANY (ARRAY['manual'::text, 'directory'::text])),
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL REFERENCES public.tenants(id) ON DELETE RESTRICT,
    PRIMARY KEY (group_id, admin_id, source)
);

CREATE INDEX iam_group_members_admin_idx ON public.iam_group_members USING btree (admin_id);

CREATE TABLE public.iam_policies (
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL REFERENCES public.tenants(id) ON DELETE RESTRICT,
    name text NOT NULL CHECK (name ~ '^[a-z][a-z0-9-]{0,63}$'),
    description text DEFAULT ''::text NOT NULL,
    statements jsonb NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    PRIMARY KEY (tenant_id, name)
);

CREATE TABLE public.service_accounts (
    id uuid DEFAULT gen_random_uuid() PRIMARY KEY,
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL REFERENCES public.tenants(id) ON DELETE RESTRICT,
    owner_id uuid NOT NULL REFERENCES public.admin_users(id) ON DELETE CASCADE,
    name text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 100),
    -- What the token starts with: unique, not secret.
    access_key text NOT NULL UNIQUE,
    -- SHA-256 of the secret (a random 32 bytes): shown once, never stored.
    secret_hash text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    last_used_at timestamp with time zone,
    expires_at timestamp with time zone,
    UNIQUE (tenant_id, owner_id, name)
);

CREATE TABLE public.iam_attachments (
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL REFERENCES public.tenants(id) ON DELETE RESTRICT,
    principal_kind text NOT NULL CHECK (principal_kind = ANY (ARRAY['user'::text, 'group'::text, 'service_account'::text])),
    principal_id uuid NOT NULL,
    policy text NOT NULL,
    PRIMARY KEY (tenant_id, principal_kind, principal_id, policy)
);

-- A principal that goes takes its attachments with it.
CREATE FUNCTION public.iam_detach() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    DELETE FROM public.iam_attachments
     WHERE principal_kind = TG_ARGV[0] AND principal_id = OLD.id;
    RETURN NULL;
END;
$$;

CREATE TRIGGER iam_detach AFTER DELETE ON public.admin_users FOR EACH ROW EXECUTE FUNCTION public.iam_detach('user');
CREATE TRIGGER iam_detach AFTER DELETE ON public.iam_groups FOR EACH ROW EXECUTE FUNCTION public.iam_detach('group');
CREATE TRIGGER iam_detach AFTER DELETE ON public.service_accounts FOR EACH ROW EXECUTE FUNCTION public.iam_detach('service_account');

-- What someone can do changed: the Actors kept in memory are reloaded.
CREATE TRIGGER notify_permissions AFTER INSERT OR DELETE OR UPDATE ON public.iam_groups FOR EACH ROW EXECUTE FUNCTION public.notify_permissions();
CREATE TRIGGER notify_permissions AFTER INSERT OR DELETE OR UPDATE ON public.iam_group_members FOR EACH ROW EXECUTE FUNCTION public.notify_permissions();
CREATE TRIGGER notify_permissions AFTER INSERT OR DELETE OR UPDATE ON public.iam_policies FOR EACH ROW EXECUTE FUNCTION public.notify_permissions();
CREATE TRIGGER notify_permissions AFTER INSERT OR DELETE OR UPDATE ON public.iam_attachments FOR EACH ROW EXECUTE FUNCTION public.notify_permissions();
CREATE TRIGGER notify_permissions AFTER DELETE ON public.service_accounts FOR EACH ROW EXECUTE FUNCTION public.notify_permissions();

ALTER TABLE public.iam_groups ENABLE ROW LEVEL SECURITY;
ALTER TABLE ONLY public.iam_groups FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON public.iam_groups USING ((tenant_id = public.current_tenant())) WITH CHECK ((tenant_id = public.current_tenant()));
ALTER TABLE public.iam_group_members ENABLE ROW LEVEL SECURITY;
ALTER TABLE ONLY public.iam_group_members FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON public.iam_group_members USING ((tenant_id = public.current_tenant())) WITH CHECK ((tenant_id = public.current_tenant()));
ALTER TABLE public.iam_policies ENABLE ROW LEVEL SECURITY;
ALTER TABLE ONLY public.iam_policies FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON public.iam_policies USING ((tenant_id = public.current_tenant())) WITH CHECK ((tenant_id = public.current_tenant()));
ALTER TABLE public.iam_attachments ENABLE ROW LEVEL SECURITY;
ALTER TABLE ONLY public.iam_attachments FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON public.iam_attachments USING ((tenant_id = public.current_tenant())) WITH CHECK ((tenant_id = public.current_tenant()));
ALTER TABLE public.service_accounts ENABLE ROW LEVEL SECURITY;
ALTER TABLE ONLY public.service_accounts FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON public.service_accounts USING ((tenant_id = public.current_tenant())) WITH CHECK ((tenant_id = public.current_tenant()));
