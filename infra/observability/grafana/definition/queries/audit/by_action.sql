-- Events per action (the permission or job it was for; sign-ins have none).
SELECT coalesce(a.action, a.event_type) AS action, count(*) AS events
FROM public.admin_audit_events a
WHERE $__timeFilter(a.occurred_at)
  AND ('__all' IN (${event_type:sqlstring}) OR a.event_type IN (${event_type:sqlstring}))
  AND ('__all' IN (${who:sqlstring}) OR a.admin_id::text IN (${who:sqlstring}))
  AND (${resource:sqlstring} = '' OR strpos(a.resource, ${resource:sqlstring}) > 0)
GROUP BY 1
ORDER BY 2 DESC, 1
LIMIT 20
