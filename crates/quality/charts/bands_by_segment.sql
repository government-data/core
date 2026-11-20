-- The entities of each segment by band in the cut (decision 0057), the
-- largest segment first.
SELECT segment,
       count(*) FILTER (WHERE band = 'high') AS high,
       count(*) FILTER (WHERE band = 'medium') AS medium,
       count(*) FILTER (WHERE band = 'low') AS low
  FROM quality.entity_icd
 WHERE dataset = :dataset AND cut = :cut::date AND band IS NOT NULL
 GROUP BY segment
 ORDER BY count(*) DESC, segment
