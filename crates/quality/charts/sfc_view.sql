-- The SFC view (decision 0058, the guide's formulas §4 and §5): each cell
-- with its share of active records right, whether it passes (90 %), its
-- missing and margin, its closeness to the threshold, and the dataset's
-- SFC, the share of its cells that pass. The highest share first.
SELECT cell, ok, total,
       round((ok::float8 / NULLIF(total, 0))::numeric, 4) AS p,
       passes, missing, margin,
       CASE
           WHEN total = 0 THEN NULL
           WHEN ok >= 0.92 * total THEN 0.2
           WHEN ok >= 0.90 * total THEN 0.5
           WHEN ok >= 0.80 * total THEN 1.0
           WHEN ok >= 0.70 * total THEN 0.6
           ELSE 0.3
       END AS closeness,
       round(avg(passes::int) OVER ()::numeric, 4) AS sfc
  FROM quality.cells
 WHERE dataset = :dataset AND cut = :cut::date
 ORDER BY ok::float8 / NULLIF(total, 0) DESC NULLS LAST, cell
