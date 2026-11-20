-- Each reported run: how long its evaluation took and how many records it
-- read, at the time it reported.
SELECT
  r.reported_at AS time,
  r.duration_ms AS duration,
  r.records
FROM quality.run_summary r
WHERE $__timeFilter(r.reported_at)
  AND ('__all' IN (${dataset:sqlstring}) OR r.dataset IN (${dataset:sqlstring}))
ORDER BY 1
