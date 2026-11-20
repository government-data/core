-- The run's jobs: its extraction and its evaluation (a finished job keeps
-- no worker: the queue clears it).
SELECT
  j.kind,
  j.state,
  j.engine,
  j.worker,
  j.attempts,
  j.started_at AS started,
  j.finished_at AS finished,
  extract(epoch FROM j.finished_at - j.started_at) AS took
FROM quality.cuts c
JOIN quality.jobs j ON j.id IN (c.extract_job, c.evaluate_job)
WHERE c.dataset = ${dataset:sqlstring}
  AND c.cut::text = ${cut:sqlstring}
ORDER BY j.created_at
