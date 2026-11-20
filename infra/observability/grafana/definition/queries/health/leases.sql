-- Jobs running with a live lease, by the worker slot that holds them
-- (`<client>/<slot>`, or `<client>/<machine>/<slot>` with a certificate).
SELECT
  j.worker,
  j.kind,
  j.dataset,
  j.engine,
  j.started_at AS started,
  j.lease_until
FROM quality.jobs j
WHERE j.state = 'running' AND j.lease_until > now()
ORDER BY j.started_at
