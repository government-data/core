-- The queue's role (decision 0023): workers connect as `qd_queue`, which can
-- only take, renew and finish jobs, of every tenant. The installer creates
-- the role (infra/postgres/initdb/20-queue-role.sh); this migration needs it.
-- Row security stays forced: the role gets a policy of its own, and the
-- application's role keeps seeing one tenant's jobs.
GRANT USAGE ON SCHEMA quality TO qd_queue;
GRANT SELECT, UPDATE (state, worker, lease_until, attempts, not_before, error, started_at, finished_at)
    ON quality.jobs TO qd_queue;
CREATE POLICY queue ON quality.jobs TO qd_queue USING (true) WITH CHECK (true);

-- A job whose lease ran out goes back to the queue, after a wait that
-- doubles with each attempt, or fails once it has used its attempts.
CREATE FUNCTION quality.release_expired(p_backoff interval) RETURNS integer
    LANGUAGE sql AS $$
    WITH expired AS (
        UPDATE quality.jobs
        SET state = CASE WHEN attempts >= max_attempts THEN 'failed' ELSE 'queued' END,
            not_before = now() + p_backoff * power(2, greatest(attempts - 1, 0)),
            worker = NULL,
            lease_until = NULL,
            error = 'lease expired',
            finished_at = CASE WHEN attempts >= max_attempts THEN now() END
        WHERE state = 'running' AND lease_until < now()
        RETURNING 1
    )
    SELECT count(*)::integer FROM expired
$$;

-- Takes the next ready job of these kinds, if any, for this worker.
CREATE FUNCTION quality.claim_job(p_worker text, p_kinds text[], p_lease interval, p_backoff interval)
    RETURNS TABLE (tenant_id uuid, id uuid, kind text, dataset text, parameters jsonb, engine text, attempts integer)
    LANGUAGE plpgsql AS $$
BEGIN
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

-- Extends the lease of a job this worker still holds; false if it lost it.
CREATE FUNCTION quality.renew_job(p_tenant uuid, p_id uuid, p_worker text, p_lease interval) RETURNS boolean
    LANGUAGE sql AS $$
    WITH renewed AS (
        UPDATE quality.jobs SET lease_until = now() + p_lease
        WHERE tenant_id = p_tenant AND id = p_id AND worker = p_worker AND state = 'running'
        RETURNING 1
    )
    SELECT EXISTS (SELECT 1 FROM renewed)
$$;

-- Finishes a job this worker holds: done, or failed and back to the queue
-- after its wait while it has attempts left. False if it lost the job.
CREATE FUNCTION quality.finish_job(p_tenant uuid, p_id uuid, p_worker text, p_error text, p_backoff interval)
    RETURNS boolean
    LANGUAGE sql AS $$
    WITH finished AS (
        UPDATE quality.jobs
        SET state = CASE WHEN p_error IS NULL THEN 'done'
                         WHEN attempts >= max_attempts THEN 'failed' ELSE 'queued' END,
            not_before = CASE WHEN p_error IS NULL THEN not_before
                              ELSE now() + p_backoff * power(2, greatest(attempts - 1, 0)) END,
            finished_at = CASE WHEN p_error IS NULL OR attempts >= max_attempts THEN now() END,
            worker = NULL,
            lease_until = NULL,
            error = p_error
        WHERE tenant_id = p_tenant AND id = p_id AND worker = p_worker AND state = 'running'
        RETURNING 1
    )
    SELECT EXISTS (SELECT 1 FROM finished)
$$;

GRANT EXECUTE ON FUNCTION quality.release_expired(interval), quality.claim_job(text, text[], interval, interval),
    quality.renew_job(uuid, uuid, text, interval), quality.finish_job(uuid, uuid, text, text, interval) TO qd_queue;
