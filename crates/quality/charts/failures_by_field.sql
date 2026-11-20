-- Fields with the most failures (decision 0031): the cut's rules by field.
SELECT field, count(*) AS rules, sum(applies)::bigint AS applies,
       sum(failures)::bigint AS failures
  FROM quality.rule_results
 WHERE dataset = :dataset AND cut = :cut::date
 GROUP BY field
 ORDER BY failures DESC, field NULLS LAST
