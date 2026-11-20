-- The entities' ICD in the cut (decision 0057), in bins of 0.05: how many
-- from 0.00, 0.05, … 0.95 (1.0 counts in the last).
SELECT round((least(floor(icd * 20), 19) / 20)::numeric, 2) AS from_icd, count(*) AS entities
  FROM quality.entity_icd
 WHERE dataset = :dataset AND cut = :cut::date AND icd IS NOT NULL
 GROUP BY 1
 ORDER BY 1
