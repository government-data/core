-- Events per dataset, from resources named dataset/, catalog/ or pipeline/<dataset>.
SELECT substring(a.resource FROM '^(?:dataset|catalog|pipeline)/([^/]+)') AS dataset, count(*) AS events
FROM public.admin_audit_events a
WHERE $__timeFilter(a.occurred_at)
  AND ('__all' IN (${event_type:sqlstring}) OR a.event_type IN (${event_type:sqlstring}))
  AND ('__all' IN (${who:sqlstring}) OR a.admin_id::text IN (${who:sqlstring}))
  AND (${resource:sqlstring} = '' OR strpos(a.resource, ${resource:sqlstring}) > 0)
  AND a.resource ~ '^(dataset|catalog|pipeline)/[^/]+'
GROUP BY 1
ORDER BY 2 DESC, 1
LIMIT 20
