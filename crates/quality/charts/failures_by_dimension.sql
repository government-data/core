-- Failures by dimension and severity (decision 0031): the cut's rules by
-- what they measure and how much a failure weighs.
SELECT dimension, severity, count(*) AS rules, sum(applies)::bigint AS applies,
       sum(failures)::bigint AS failures
  FROM quality.rule_results
 WHERE dataset = :dataset AND cut = :cut::date
 GROUP BY dimension, severity
 ORDER BY failures DESC, dimension NULLS LAST, severity NULLS LAST
