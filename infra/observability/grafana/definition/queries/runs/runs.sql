-- Each run of a cut: its two jobs (a finished job keeps no worker: the
-- queue clears it), its summary and the conformity of its rules. The
-- hidden log_from and log_to give the link to logs by run its time range:
-- ten minutes either side of the run.
SELECT
  c.fired_at AS fired,
  c.dataset,
  c.cut::text AS cut,
  c.run,
  c.state,
  extract(epoch FROM c.fired_at - c.due_at) AS lateness,
  ev.engine,
  ev.state AS evaluate_state,
  ev.worker,
  ev.attempts,
  extract(epoch FROM ex.finished_at - ex.started_at) AS extract_time,
  extract(epoch FROM ev.finished_at - ev.started_at) AS evaluate_time,
  r.records,
  r.duration_ms AS duration,
  r.igcd,
  q.conformity,
  (extract(epoch FROM c.fired_at) * 1000)::bigint - 600000 AS log_from,
  (extract(epoch FROM coalesce(greatest(ex.finished_at, ev.finished_at), now())) * 1000)::bigint
    + 600000 AS log_to
FROM quality.cuts c
LEFT JOIN quality.jobs ex ON ex.id = c.extract_job
LEFT JOIN quality.jobs ev ON ev.id = c.evaluate_job
LEFT JOIN quality.run_summary r ON r.dataset = c.dataset AND r.cut = c.cut
LEFT JOIN LATERAL (
  SELECT 1 - sum(rr.failures)::double precision / nullif(sum(rr.applies), 0) AS conformity
  FROM quality.rule_results rr
  WHERE rr.dataset = c.dataset AND rr.cut = c.cut
) q ON true
WHERE $__timeFilter(c.fired_at)
  AND ('__all' IN (${dataset:sqlstring}) OR c.dataset IN (${dataset:sqlstring}))
ORDER BY c.fired_at DESC
LIMIT 500
