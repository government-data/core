-- Remediation queues (decision 0059): per dimension, its fifteen queues
-- with the most open findings at the cut, each with those overdue by its
-- SLA (its group's, else its dimension's, else 30 days), those resolved
-- at the cut, its oldest open finding's age in days since the cut it
-- opened at, and the open ones' summed priority; the empty group as ''.
SELECT dimension, grp, open, overdue, resolved, oldest_days, sla_days,
       round(priority::numeric, 4) AS priority
  FROM (SELECT q.*, row_number() OVER (PARTITION BY dimension ORDER BY open DESC, grp) AS n
          FROM (SELECT f.dimension, f.grp,
                       sum(f.open)::bigint AS open,
                       coalesce(sum(f.open) FILTER (
                           WHERE f.since + coalesce(g.days, d.days, 30) < :cut::date), 0)::bigint
                           AS overdue,
                       sum(f.resolved)::bigint AS resolved,
                       max(:cut::date - f.since) FILTER (WHERE f.open > 0) AS oldest_days,
                       coalesce(max(g.days), max(d.days), 30) AS sla_days,
                       sum(f.priority) AS priority
                  FROM quality.queue_findings f
                  LEFT JOIN quality.remediation_slas g
                    ON g.tenant_id = f.tenant_id AND g.dataset = f.dataset
                   AND g.dimension = f.dimension AND g.grp = f.grp
                  LEFT JOIN quality.remediation_slas d
                    ON d.tenant_id = f.tenant_id AND d.dataset = f.dataset
                   AND d.dimension = f.dimension AND d.grp IS NULL
                 WHERE f.dataset = :dataset AND f.cut = :cut::date
                 GROUP BY f.dimension, f.grp) q) r
 WHERE n <= 15
 ORDER BY dimension, open DESC, grp
