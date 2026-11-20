-- Outliers by group (decision 0031, the guide's formula 7): per dimension,
-- the fifteen groups with the most outliers of the outlier amount, robust
-- z beyond 3.5 within the group, each with its share; the empty group as ''.
SELECT variable, dimension, grp, records, low, high,
       round(((low + high)::float8 / NULLIF(records, 0))::numeric, 4) AS share
  FROM (SELECT *, row_number() OVER (PARTITION BY variable, dimension
                                         ORDER BY low + high DESC, records DESC, grp) AS n
          FROM quality.distribution_checks
         WHERE dataset = :dataset AND cut = :cut::date AND dimension <> '') d
 WHERE n <= 15
 ORDER BY dimension, low + high DESC, records DESC, grp
