-- How the entities moved between bands from the previous cut to this one
-- (decision 0057), matched by their hash; none without a previous cut.
SELECT p.band AS from_band, c.band AS to_band, count(*) AS entities
  FROM quality.entity_icd c
  JOIN quality.entity_icd p
    ON p.tenant_id = c.tenant_id AND p.dataset = c.dataset AND p.entity = c.entity
   AND p.cut = :previous_cut::date
 WHERE c.dataset = :dataset AND c.cut = :cut::date
   AND c.band IS NOT NULL AND p.band IS NOT NULL
 GROUP BY 1, 2
 ORDER BY array_position(ARRAY['high', 'medium', 'low'], p.band),
          array_position(ARRAY['high', 'medium', 'low'], c.band)
