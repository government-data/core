-- Maintenance stops handing out jobs (decision 0003, the user's ruling of
-- 2026-11-21): while the installation is in maintenance no queued job is
-- claimed, so jobs queued do not hold an update back; they wait and are
-- claimed once it ends. A job already running goes on, and is what the
-- update agent waits for. Only the queue's role may ask.
GRANT EXECUTE ON FUNCTION public.in_maintenance() TO qd_queue;

-- The queue's claim, as in 0026_jobs_uses_trino.sql, taking nothing during
-- maintenance.
CREATE OR REPLACE FUNCTION quality.claim_job(p_worker text, p_kinds text[], p_lease interval, p_backoff interval)
    RETURNS TABLE (tenant_id uuid, id uuid, kind text, dataset text, parameters jsonb, engine text, attempts integer)
    LANGUAGE plpgsql AS $$
BEGIN
    PERFORM quality.queue_check(true);
    PERFORM pg_catalog.pg_advisory_xact_lock_shared(6600009);
    PERFORM quality.release_expired(p_backoff);
    RETURN QUERY
    WITH next AS (
        SELECT j.tenant_id, j.id FROM quality.jobs j
        WHERE j.state = 'queued' AND j.not_before <= now() AND j.kind = ANY (p_kinds)
          AND NOT public.in_maintenance()
          AND NOT (quality.on_trino(j.kind, j.uses_trino) AND EXISTS (
              SELECT 1 FROM quality.trino_holds h
               WHERE h.tenant_id = j.tenant_id AND h.expires_at > now()))
        ORDER BY j.priority DESC, j.created_at
        FOR UPDATE OF j SKIP LOCKED
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
