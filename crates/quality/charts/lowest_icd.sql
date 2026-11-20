-- Fields with the lowest ICD in the cut (decision 0057): each field's mean
-- over the records where it has one, the lowest first.
SELECT field, round(mean_icd::numeric, 4) AS icd
  FROM quality.field_quality
 WHERE dataset = :dataset AND cut = :cut::date AND mean_icd IS NOT NULL
 ORDER BY mean_icd, field
