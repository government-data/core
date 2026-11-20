-- Largest groups (decision 0031): per dimension, its fifteen groups with
-- the most records, each with its share of serious failures and its mean
-- ICD; the empty group too, as ''.
SELECT dimension, grp, records, serious,
       round((serious::float8 / NULLIF(records, 0))::numeric, 4) AS serious_share,
       round(mean_icd::numeric, 4) AS icd
  FROM (SELECT *, row_number() OVER (PARTITION BY dimension ORDER BY records DESC, grp) AS n
          FROM quality.group_metrics
         WHERE dataset = :dataset AND cut = :cut::date) g
 WHERE n <= 15
 ORDER BY dimension, records DESC, grp
