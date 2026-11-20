-- PSI by deciles (decision 0031, the guide's formula 8): per drift
-- variable, the previous cut's deciles as bins, each cut's share in them
-- read off its percentiles (to 1 %), and the PSI their sum; none without
-- a previous cut. Bins empty in both count with a floor of 0.0001.
WITH c AS (
    SELECT variable, percentiles AS cur FROM quality.distribution_checks
     WHERE dataset = :dataset AND cut = :cut::date AND dimension = ''
       AND percentiles IS NOT NULL),
p AS (
    SELECT variable, percentiles AS prev FROM quality.distribution_checks
     WHERE dataset = :dataset AND cut = :previous_cut::date AND dimension = ''
       AND percentiles IS NOT NULL),
f AS (
    SELECT c.variable, k,
           CASE WHEN k = 0 THEN 0 WHEN k = 10 THEN 1
                ELSE (SELECT greatest(count(*) - 1, 0) FROM unnest(c.cur) x
                       WHERE x <= p.prev[10 * k + 1]) / 100.0 END AS fa,
           CASE WHEN k = 0 THEN 0 WHEN k = 10 THEN 1
                ELSE (SELECT greatest(count(*) - 1, 0) FROM unnest(p.prev) x
                       WHERE x <= p.prev[10 * k + 1]) / 100.0 END AS fe
      FROM c JOIN p USING (variable), generate_series(0, 10) AS k),
b AS (
    SELECT variable, k,
           greatest(fe - lag(fe) OVER w, 0.0001) AS e,
           greatest(fa - lag(fa) OVER w, 0.0001) AS a
      FROM f WINDOW w AS (PARTITION BY variable ORDER BY k))
SELECT variable, k AS decile, round(e, 4) AS base, round(a, 4) AS now,
       round(sum((a - e) * ln(a / e)) OVER (PARTITION BY variable), 4) AS psi
  FROM b
 WHERE k > 0
 ORDER BY variable, k
