-- Time per stage of each reported run's evaluation, at the time it
-- reported; a stage's name as the worker reports it.
SELECT
  r.reported_at AS time,
  s.stage AS metric,
  s.ms AS value
FROM quality.job_stats s
JOIN quality.run_summary r ON r.dataset = s.dataset AND r.cut = s.cut
WHERE $__timeFilter(r.reported_at)
  AND ('__all' IN (${dataset:sqlstring}) OR r.dataset IN (${dataset:sqlstring}))
ORDER BY 1, s.position
