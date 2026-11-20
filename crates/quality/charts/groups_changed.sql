-- Groups that changed most (decision 0031): per dimension, the fifteen
-- groups whose share of serious failures moved most since the previous
-- cut, each in both; none without a previous cut.
SELECT dimension, grp, before, now, round((now - before)::numeric, 4) AS change
  FROM (SELECT c.dimension, c.grp,
               round((p.serious::float8 / p.records)::numeric, 4) AS before,
               round((c.serious::float8 / c.records)::numeric, 4) AS now,
               row_number() OVER (PARTITION BY c.dimension
                   ORDER BY abs(c.serious::float8 / c.records - p.serious::float8 / p.records) DESC,
                            c.grp) AS n
          FROM quality.group_metrics c
          JOIN quality.group_metrics p
            ON p.tenant_id = c.tenant_id AND p.dataset = c.dataset
           AND p.dimension = c.dimension AND p.grp = c.grp AND p.cut = :previous_cut::date
         WHERE c.dataset = :dataset AND c.cut = :cut::date
           AND c.records > 0 AND p.records > 0) g
 WHERE n <= 15
 ORDER BY dimension, abs(now - before) DESC, grp
