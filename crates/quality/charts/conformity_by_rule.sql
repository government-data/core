-- Conformity by rule (decision 0031): each rule of the cut, the worst first.
SELECT rule, field, dimension, severity, applies, failures,
       CASE WHEN applies > 0 THEN round(1 - failures::numeric / applies, 4) END AS conformity
  FROM quality.rule_results
 WHERE dataset = :dataset AND cut = :cut::date
 ORDER BY conformity NULLS LAST, rule
