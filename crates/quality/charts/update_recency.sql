-- Update recency (decision 0031): the records of the cut by the age of its
-- `updated` field at the cut, in its bands, the empty ones last.
SELECT r.band, r.records
  FROM quality.field_quality f,
       LATERAL (SELECT 'b' || o AS band, n AS records, o
                  FROM unnest(f.recency) WITH ORDINALITY AS u (n, o)
                UNION ALL
                SELECT 'empty', f.empty, 99) r
 WHERE f.dataset = :dataset AND f.cut = :cut::date AND f.recency IS NOT NULL
 ORDER BY r.o
