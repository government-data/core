-- Jobs finished per hour: done, and failed for good (a failed attempt
-- that goes back to the queue has no finish time).
SELECT
  $__timeGroupAlias(j.finished_at, '1h'),
  count(*) FILTER (WHERE j.state = 'done') AS done,
  count(*) FILTER (WHERE j.state = 'failed') AS failed
FROM quality.jobs j
WHERE $__timeFilter(j.finished_at)
GROUP BY 1
ORDER BY 1
