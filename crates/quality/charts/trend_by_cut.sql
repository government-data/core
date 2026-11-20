-- Records, conformity and the IGCD by cut (decisions 0031 and 0057): the
-- last twelve cuts up to this one, oldest first.
SELECT cut, records, failures, conformity, igcd
  FROM (SELECT s.cut::text AS cut, s.records, round(s.igcd::numeric, 4) AS igcd,
               COALESCE(sum(r.failures), 0)::bigint AS failures,
               CASE WHEN sum(r.applies) > 0
                    THEN round(1 - sum(r.failures)::numeric / sum(r.applies), 4) END AS conformity
          FROM quality.run_summary s
          LEFT JOIN quality.rule_results r USING (tenant_id, dataset, cut)
         WHERE s.dataset = :dataset AND s.cut <= :cut::date
         GROUP BY s.cut, s.records, s.igcd
         ORDER BY s.cut DESC
         LIMIT 12) recent
 ORDER BY cut
