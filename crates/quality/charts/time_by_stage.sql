-- Time per stage (decision 0031, the run log of 0026): how long each stage
-- of the cut's evaluation took, in the order it ran, and its milliseconds
-- per million record-attributes (the guide's formula 9, V = records ×
-- mapped fields), the previous cut's beside it; no rate without a volume.
WITH volume AS (
    SELECT s.cut, s.records * count(f.field) AS v
      FROM quality.run_summary s
      LEFT JOIN quality.field_quality f
        ON f.tenant_id = s.tenant_id AND f.dataset = s.dataset AND f.cut = s.cut
     WHERE s.dataset = :dataset AND s.cut IN (:cut::date, :previous_cut::date)
     GROUP BY s.cut, s.records
), times AS (
    SELECT j.cut, j.stage, j.position, j.ms,
           CASE WHEN v.v > 0 THEN round(j.ms * 1e6 / v.v, 2) END AS per_million
      FROM quality.job_stats j
      JOIN volume v ON v.cut = j.cut
     WHERE j.dataset = :dataset
)
SELECT c.stage, c.ms, c.per_million,
       p.ms AS previous_ms, p.per_million AS previous_per_million
  FROM times c
  LEFT JOIN times p ON p.stage = c.stage AND p.cut = :previous_cut::date
 WHERE c.cut = :cut::date
 ORDER BY c.position
