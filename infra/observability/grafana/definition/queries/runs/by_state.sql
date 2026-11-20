-- Runs fired per interval, by their state now.
SELECT
  $__timeGroupAlias(c.fired_at, $__interval),
  count(*) FILTER (WHERE c.state = 'extracting') AS extracting,
  count(*) FILTER (WHERE c.state = 'evaluating') AS evaluating,
  count(*) FILTER (WHERE c.state = 'done') AS done,
  count(*) FILTER (WHERE c.state = 'failed') AS failed
FROM quality.cuts c
WHERE $__timeFilter(c.fired_at)
  AND ('__all' IN (${dataset:sqlstring}) OR c.dataset IN (${dataset:sqlstring}))
GROUP BY 1
ORDER BY 1
