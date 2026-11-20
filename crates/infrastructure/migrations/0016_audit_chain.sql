-- The audit trail chained by hash (decision 0070, points 2 to 9 and 13).
-- Each row gets its tenant's next sequence number and the SHA-256 of its
-- fields, the previous row's hash and that number, from a trigger, so every
-- writer is chained, whatever version it runs. The encoding is computed again
-- in Rust (`domain::audit_chain`) by the verifier, never by a database
-- function; a test holds the two equal. Guards refuse update, delete and
-- truncate. They show tampering; they do not stop the table's owner.
--
-- The rows already there are sealed here in `occurred_at` then `id` order
-- (adoption): chained from now on, not proved before it.

ALTER TABLE public.admin_audit_events
    ADD COLUMN chain_seq bigint,
    ADD COLUMN chain_hash bytea,
    ADD COLUMN chain_version smallint;

CREATE UNIQUE INDEX admin_audit_events_chain_idx
    ON public.admin_audit_events (tenant_id, chain_seq);

-- One head per tenant: the last sequence number and its hash, and where
-- adoption ended.
CREATE TABLE public.audit_chain_heads (
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL PRIMARY KEY
        REFERENCES public.tenants(id) ON DELETE RESTRICT,
    seq bigint NOT NULL,
    hash bytea NOT NULL,
    adopted_seq bigint NOT NULL,
    adopted_at timestamp with time zone NOT NULL
);
ALTER TABLE public.audit_chain_heads ENABLE ROW LEVEL SECURITY;
CREATE POLICY isolation ON public.audit_chain_heads
    USING (tenant_id = public.current_tenant())
    WITH CHECK (tenant_id = public.current_tenant());

-- The encoding, version 1 (point 2).
CREATE FUNCTION public.audit_chain_genesis(tenant uuid) RETURNS bytea
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT sha256(convert_to('qd-audit-chain/genesis/' || tenant::text, 'UTF8'))
$$;

-- A field: its name, length-prefixed, then the SHA-256 of its value; a NULL
-- value is absent.
CREATE FUNCTION public.audit_chain_field(name text, value bytea) RETURNS bytea
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT CASE WHEN value IS NULL THEN ''::bytea
        ELSE int4send(octet_length(convert_to(name, 'UTF8'))) || convert_to(name, 'UTF8')
             || sha256(value) END
$$;

-- An array: its element count, then each element's length and bytes, a NULL
-- element as length -1.
CREATE FUNCTION public.audit_chain_texts(items text[]) RETURNS bytea
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT CASE WHEN items IS NULL THEN NULL
        ELSE int4send(coalesce(cardinality(items), 0)) || coalesce((
            SELECT string_agg(CASE WHEN e IS NULL THEN int4send(-1)
                ELSE int4send(octet_length(convert_to(e, 'UTF8'))) || convert_to(e, 'UTF8') END,
                ''::bytea ORDER BY o)
            FROM unnest(items) WITH ORDINALITY AS u(e, o)), ''::bytea) END
$$;

-- The bytes hashed for row `seq` after `prev`. A column added to the audit
-- later is appended here, by the migration that adds it. PL/pgSQL, not SQL:
-- PostgreSQL 16 does not inline this body, and plans an SQL function's body
-- again at every call (about 2.8 ms a row, measured), while PL/pgSQL keeps
-- its plan for the session (about 0.2 ms).
CREATE FUNCTION public.audit_row_v1(prev bytea, seq bigint, r public.admin_audit_events)
RETURNS bytea LANGUAGE plpgsql IMMUTABLE PARALLEL SAFE AS $$
BEGIN
    RETURN convert_to('qd-audit-chain/1', 'UTF8') || prev || int8send(seq)
        || public.audit_chain_field('tenant_id', convert_to(r.tenant_id::text, 'UTF8'))
        || public.audit_chain_field('id', convert_to(r.id::text, 'UTF8'))
        || public.audit_chain_field('event_type', convert_to(r.event_type, 'UTF8'))
        || public.audit_chain_field('admin_id', convert_to(r.admin_id::text, 'UTF8'))
        || public.audit_chain_field('email_attempted', convert_to(r.email_attempted, 'UTF8'))
        || public.audit_chain_field('ip', convert_to(r.ip, 'UTF8'))
        || public.audit_chain_field('user_agent', convert_to(r.user_agent, 'UTF8'))
        || public.audit_chain_field('occurred_at',
               int8send((extract(epoch FROM r.occurred_at) * 1000000)::bigint))
        || public.audit_chain_field('action', convert_to(r.action, 'UTF8'))
        || public.audit_chain_field('resource', convert_to(r.resource, 'UTF8'))
        || public.audit_chain_field('service_account', convert_to(r.service_account, 'UTF8'))
        || public.audit_chain_field('job_id', convert_to(r.job_id::text, 'UTF8'))
        || public.audit_chain_field('purpose', convert_to(r.purpose, 'UTF8'))
        || public.audit_chain_field('reference', convert_to(r.reference, 'UTF8'))
        || public.audit_chain_field('rows_shown', int8send(r.rows_shown::bigint))
        || public.audit_chain_field('fields_shown', public.audit_chain_texts(r.fields_shown));
END
$$;

-- Seals a new row under its tenant's head, locked until the row's own
-- transaction ends, so the numbers are dense (point 6). A tenant with no
-- head opens its chain only if it has no audit rows at all (point 7).
CREATE FUNCTION public.audit_chain_seal() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    head record;
BEGIN
    SELECT seq, hash INTO head FROM public.audit_chain_heads
     WHERE tenant_id = NEW.tenant_id FOR UPDATE;
    IF NOT FOUND THEN
        IF EXISTS (SELECT 1 FROM public.admin_audit_events WHERE tenant_id = NEW.tenant_id) THEN
            RAISE EXCEPTION 'audit chain: tenant % has audit rows but no head', NEW.tenant_id;
        END IF;
        INSERT INTO public.audit_chain_heads (tenant_id, seq, hash, adopted_seq, adopted_at)
        VALUES (NEW.tenant_id, 0, public.audit_chain_genesis(NEW.tenant_id), 0, now())
        ON CONFLICT (tenant_id) DO NOTHING;
        SELECT seq, hash INTO head FROM public.audit_chain_heads
         WHERE tenant_id = NEW.tenant_id FOR UPDATE;
    END IF;
    NEW.chain_seq := head.seq + 1;
    NEW.chain_version := 1;
    NEW.chain_hash := sha256(public.audit_row_v1(head.hash, NEW.chain_seq, NEW));
    UPDATE public.audit_chain_heads SET seq = NEW.chain_seq, hash = NEW.chain_hash
     WHERE tenant_id = NEW.tenant_id;
    RETURN NEW;
END
$$;

-- Adoption (point 13). The owner sees every tenant's rows only without
-- FORCE, put back below in this same transaction; the table is locked since
-- its ALTER above, so no row is written between adoption and the trigger.
-- A tenant's hashes are gathered first and written by one UPDATE, about
-- twice as fast as one UPDATE a row (100,000 rows: 36 s against 65 s).
ALTER TABLE public.admin_audit_events NO FORCE ROW LEVEL SECURITY;
DO $$
DECLARE
    t uuid;
    r public.admin_audit_events;
    prev bytea;
    n bigint;
    ids uuid[];
    hashes bytea[];
BEGIN
    FOR t IN SELECT DISTINCT tenant_id FROM public.admin_audit_events LOOP
        prev := public.audit_chain_genesis(t);
        n := 0;
        ids := '{}';
        hashes := '{}';
        FOR r IN SELECT * FROM public.admin_audit_events WHERE tenant_id = t
                  ORDER BY occurred_at, id LOOP
            n := n + 1;
            prev := sha256(public.audit_row_v1(prev, n, r));
            ids := ids || r.id;
            hashes := hashes || prev;
        END LOOP;
        UPDATE public.admin_audit_events e
           SET chain_seq = u.seq, chain_hash = u.hash, chain_version = 1
          FROM unnest(ids, hashes) WITH ORDINALITY AS u(id, hash, seq)
         WHERE e.id = u.id;
        INSERT INTO public.audit_chain_heads (tenant_id, seq, hash, adopted_seq, adopted_at)
        VALUES (t, n, prev, n, now());
    END LOOP;
END
$$;
ALTER TABLE public.admin_audit_events FORCE ROW LEVEL SECURITY;
ALTER TABLE public.audit_chain_heads FORCE ROW LEVEL SECURITY;

CREATE TRIGGER audit_chain_seal BEFORE INSERT ON public.admin_audit_events
    FOR EACH ROW EXECUTE FUNCTION public.audit_chain_seal();

-- The guards (point 9).
CREATE FUNCTION public.audit_chain_refuse() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'admin_audit_events is append-only: % refused', TG_OP;
END
$$;
CREATE TRIGGER audit_chain_guard BEFORE UPDATE OR DELETE OR TRUNCATE
    ON public.admin_audit_events
    FOR EACH STATEMENT EXECUTE FUNCTION public.audit_chain_refuse();
