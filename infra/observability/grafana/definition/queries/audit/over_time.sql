-- Audit events over time, one series per event type.
SELECT $__timeGroupAlias(a.occurred_at, $__interval), a.event_type AS metric, count(*) AS value
FROM public.admin_audit_events a
WHERE $__timeFilter(a.occurred_at)
  AND ('__all' IN (${event_type:sqlstring}) OR a.event_type IN (${event_type:sqlstring}))
  AND ('__all' IN (${who:sqlstring}) OR a.admin_id::text IN (${who:sqlstring}))
  AND (${resource:sqlstring} = '' OR strpos(a.resource, ${resource:sqlstring}) > 0)
GROUP BY 1, 2
ORDER BY 1, 2
