-- The queue now, by kind: ready to take, waiting for their backoff,
-- running with a live lease, and running with a lease run out (back to the
-- queue at the next claim).
SELECT
  j.kind,
  count(*) FILTER (WHERE j.state = 'queued' AND j.not_before <= now()) AS ready,
  count(*) FILTER (WHERE j.state = 'queued' AND j.not_before > now()) AS waiting,
  count(*) FILTER (WHERE j.state = 'running' AND j.lease_until > now()) AS running,
  count(*) FILTER (WHERE j.state = 'running' AND j.lease_until <= now()) AS lease_expired
FROM quality.jobs j
WHERE j.state IN ('queued', 'running')
GROUP BY j.kind
ORDER BY j.kind
