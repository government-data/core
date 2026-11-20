-- p′ chart by group (decision 0031, the guide's formulas, Laney): per
-- dimension, each group's share of records with a serious failure against
-- the dimension's, with limits widened by the real spread between groups
-- (the moving range of z, groups by size). The empty group is left out.
WITH g AS (
    SELECT dimension, grp, records, serious, serious::float8 / records AS p
      FROM quality.group_metrics
     WHERE dataset = :dataset AND cut = :cut::date AND grp <> '' AND records > 0),
pb AS (
    SELECT dimension, sum(serious)::float8 / sum(records) AS p_bar FROM g GROUP BY 1),
z AS (
    SELECT g.*, pb.p_bar, sqrt(pb.p_bar * (1 - pb.p_bar) / g.records) AS sigma
      FROM g JOIN pb USING (dimension)),
mr AS (
    SELECT *, abs(z - lag(z) OVER (PARTITION BY dimension ORDER BY records, grp)) AS mr
      FROM (SELECT *, (p - p_bar) / NULLIF(sigma, 0) AS z FROM z) zz),
sz AS (
    SELECT dimension, COALESCE(avg(mr) / 1.128, 1) AS sigma_z FROM mr GROUP BY 1),
l AS (
    SELECT mr.*, greatest(0, p_bar - 3 * sigma * sigma_z) AS lcl,
           least(1, p_bar + 3 * sigma * sigma_z) AS ucl
      FROM mr JOIN sz USING (dimension))
SELECT dimension, grp, records, serious, round(p::numeric, 4) AS p,
       round(p_bar::numeric, 4) AS p_bar, round(lcl::numeric, 4) AS lcl,
       round(ucl::numeric, 4) AS ucl,
       CASE WHEN p > ucl THEN 'above' WHEN p < lcl THEN 'below' ELSE 'in' END AS signal
  FROM l
 ORDER BY dimension, records DESC, grp
