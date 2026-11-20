-- Views of personal data over time, one series per kind of view (source, dataset, lake).
SELECT $__timeGroupAlias(a.occurred_at, $__interval), split_part(a.resource, '/', 1) AS metric, count(*) AS value
FROM public.admin_audit_events a
WHERE $__timeFilter(a.occurred_at)
  AND a.event_type = 'records_viewed'
  AND ('__all' IN (${who:sqlstring}) OR a.admin_id::text IN (${who:sqlstring}))
  AND (${resource:sqlstring} = '' OR strpos(a.resource, ${resource:sqlstring}) > 0)
GROUP BY 1, 2
ORDER BY 1, 2
