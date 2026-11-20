-- Which machine a queue session is, and whether it may work (decision 0066,
-- points 4 and 6). PostgreSQL lets any worker's certificate of the
-- installation's CA in as `qd_queue` (pg_hba.conf, `cert` with a map),
-- whatever the machine's state: it cannot ask anyone at the handshake. So
-- the queue asks. A session with a client certificate is the machine of its
-- CN, and it works only while that machine is approved (it takes, renews and
-- finishes jobs) or draining (it renews and finishes the ones it holds, and
-- takes none). A pending, retired or unknown machine gets an error from the
-- queue's functions, and sees no job through the table either (the policy
-- `queue_machine`). A session without a client certificate (development's password)
-- works as before.

-- The worker named by this session's certificate: NULL without a client
-- certificate, '' for one that is not a worker's. As the caller, who sees
-- its own session's certificate; every name qualified, so that no temporary
-- object stands for one.
CREATE FUNCTION quality.queue_session_machine() RETURNS text
    LANGUAGE sql STABLE SET search_path = pg_catalog, pg_temp AS $$
    SELECT CASE WHEN s.client_dn IS NULL THEN NULL
                ELSE coalesce(pg_catalog.substring(s.client_dn, '^/O=qd/OU=worker/CN=([a-z0-9-]{1,63})$'), '')
           END
      FROM pg_catalog.pg_stat_ssl s
     WHERE s.pid = pg_catalog.pg_backend_pid()
$$;

-- The state of the worker named `p_name` that is not retired, or 'unknown'.
-- As the owner: the queue's role may not read `machines`, whose rows are a
-- tenant's; an installation has one (0065), and each tenant is looked in,
-- whatever the caller set. `app.tenant_id` is set for the transaction and
-- put back before returning (a function's own `SET` of it is refused to a
-- role that is not a superuser).
CREATE FUNCTION quality.queue_machine_state(p_name text) RETURNS text
    LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, pg_temp AS $$
DECLARE
    v_was text := pg_catalog.current_setting('app.tenant_id', true);
    v_tenant uuid;
    v_state text := 'unknown';
BEGIN
    FOR v_tenant IN SELECT t.id FROM public.tenants t ORDER BY t.id LOOP
        PERFORM pg_catalog.set_config('app.tenant_id', v_tenant::text, true);
        SELECT m.state INTO v_state FROM public.machines m
         WHERE m.name = p_name AND m.role = 'worker' AND m.state <> 'retired';
        EXIT WHEN FOUND;
        v_state := 'unknown';
    END LOOP;
    PERFORM pg_catalog.set_config('app.tenant_id', coalesce(v_was, ''), true);
    RETURN v_state;
END
$$;
REVOKE ALL ON FUNCTION quality.queue_machine_state(text) FROM PUBLIC;

-- This session's machine: NULL without a client certificate, otherwise its
-- state or 'unknown'.
CREATE FUNCTION quality.queue_session_state() RETURNS text
    LANGUAGE sql SET search_path = pg_catalog, pg_temp AS $$
    SELECT CASE WHEN s.m IS NULL THEN NULL
                WHEN s.m = '' THEN 'unknown'
                ELSE quality.queue_machine_state(s.m)
           END
      FROM (SELECT quality.queue_session_machine() AS m) s
$$;

-- Whether a session in `p_session` (queue_session_state) may touch a job in
-- `p_job` ('queued' to take it, 'running' to renew or finish it).
CREATE FUNCTION quality.queue_allows(p_session text, p_job text) RETURNS boolean
    LANGUAGE sql IMMUTABLE AS $$
    SELECT p_session IS NULL OR p_session = 'approved' OR (p_session = 'draining' AND p_job = 'running')
$$;

-- Raises, saying why, unless this session's machine may take jobs
-- (`p_take`) or work on the ones it holds.
CREATE FUNCTION quality.queue_check(p_take boolean) RETURNS void
    LANGUAGE plpgsql SET search_path = pg_catalog, pg_temp AS $$
DECLARE
    v_state text := quality.queue_session_state();
    v_name text := quality.queue_session_machine();
BEGIN
    IF quality.queue_allows(v_state, CASE WHEN p_take THEN 'queued' ELSE 'running' END) THEN
        RETURN;
    END IF;
    RAISE EXCEPTION USING ERRCODE = 'insufficient_privilege', MESSAGE = CASE v_state
        WHEN 'pending' THEN format('machine %s is pending: it takes no jobs until it is approved', v_name)
        WHEN 'draining' THEN format('machine %s is draining: it finishes its jobs and takes no new one', v_name)
        ELSE format('the certificate (%s) is not a live worker''s: it takes no jobs', coalesce(nullif(v_name, ''), 'not a worker'))
    END;
END
$$;

GRANT EXECUTE ON FUNCTION quality.queue_session_machine(), quality.queue_machine_state(text),
    quality.queue_session_state(), quality.queue_allows(text, text), quality.queue_check(boolean) TO qd_queue;

-- The table itself, for a session that would skip the functions: a machine
-- that may not work (pending, retired, unknown) sees and changes no job.
-- Restrictive, so it holds on top of every permissive policy (`queue`, and
-- the tenants' `isolation`, which alone would show the default tenant's).
-- That a draining machine takes no new job is the functions' to hold: a
-- row's new state is checked against the same expression as the rows seen,
-- and its job going back to the queue on an error must pass.
CREATE POLICY queue_machine ON quality.jobs AS RESTRICTIVE TO qd_queue
    USING (quality.queue_allows((SELECT quality.queue_session_state()), 'running'))
    WITH CHECK (quality.queue_allows((SELECT quality.queue_session_state()), 'running'));

-- The queue's functions, as in 0002_queue.sql, checking first.
CREATE OR REPLACE FUNCTION quality.claim_job(p_worker text, p_kinds text[], p_lease interval, p_backoff interval)
    RETURNS TABLE (tenant_id uuid, id uuid, kind text, dataset text, parameters jsonb, engine text, attempts integer)
    LANGUAGE plpgsql AS $$
BEGIN
    PERFORM quality.queue_check(true);
    PERFORM quality.release_expired(p_backoff);
    RETURN QUERY
    WITH next AS (
        SELECT j.tenant_id, j.id FROM quality.jobs j
        WHERE j.state = 'queued' AND j.not_before <= now() AND j.kind = ANY (p_kinds)
        ORDER BY j.priority DESC, j.created_at
        FOR UPDATE SKIP LOCKED
        LIMIT 1
    )
    UPDATE quality.jobs j
    SET state = 'running', worker = p_worker, lease_until = now() + p_lease,
        attempts = j.attempts + 1, started_at = now(), error = NULL
    FROM next
    WHERE j.tenant_id = next.tenant_id AND j.id = next.id
    RETURNING j.tenant_id, j.id, j.kind, j.dataset, j.parameters, j.engine, j.attempts;
END
$$;

CREATE OR REPLACE FUNCTION quality.renew_job(p_tenant uuid, p_id uuid, p_worker text, p_lease interval)
    RETURNS boolean
    LANGUAGE plpgsql AS $$
BEGIN
    PERFORM quality.queue_check(false);
    UPDATE quality.jobs j SET lease_until = now() + p_lease
     WHERE j.tenant_id = p_tenant AND j.id = p_id AND j.worker = p_worker AND j.state = 'running';
    RETURN FOUND;
END
$$;

CREATE OR REPLACE FUNCTION quality.finish_job(p_tenant uuid, p_id uuid, p_worker text, p_error text, p_backoff interval)
    RETURNS boolean
    LANGUAGE plpgsql AS $$
BEGIN
    PERFORM quality.queue_check(false);
    UPDATE quality.jobs j
    SET state = CASE WHEN p_error IS NULL THEN 'done'
                     WHEN j.attempts >= j.max_attempts THEN 'failed' ELSE 'queued' END,
        not_before = CASE WHEN p_error IS NULL THEN j.not_before
                          ELSE now() + p_backoff * power(2, greatest(j.attempts - 1, 0)) END,
        finished_at = CASE WHEN p_error IS NULL OR j.attempts >= j.max_attempts THEN now() END,
        worker = NULL,
        lease_until = NULL,
        error = p_error
    WHERE j.tenant_id = p_tenant AND j.id = p_id AND j.worker = p_worker AND j.state = 'running';
    RETURN FOUND;
END
$$;
