-- Trino leaves Quality Data (decision 0077): no job is held for Trino's
-- coordinator any more. The queue's claim, as in
-- 0027_claim_in_maintenance.sql, without the hold: it no longer reads
-- quality.trino_holds nor quality.on_trino, and no longer takes the hold's
-- advisory lock.
--
-- quality.trino_holds, quality.jobs.uses_trino, quality.may_use_trino and
-- quality.on_trino stay, unused, until a later version drops them.
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
          AND NOT public.in_maintenance()
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
