-- Who saw clients' data: the records_viewed events, newest first (at most 1000).
SELECT a.occurred_at AS time, coalesce(u.display_name, u.email, a.service_account) AS who,
  a.action, a.resource, substring(a.resource FROM '^(?:dataset|catalog|pipeline)/([^/]+)') AS dataset,
  a.job_id::text AS job_id, a.ip, a.user_agent
FROM public.admin_audit_events a
LEFT JOIN public.admin_users u ON u.id = a.admin_id
WHERE $__timeFilter(a.occurred_at)
  AND a.event_type = 'records_viewed'
  AND ('__all' IN (${who:sqlstring}) OR a.admin_id::text IN (${who:sqlstring}))
  AND (${resource:sqlstring} = '' OR strpos(a.resource, ${resource:sqlstring}) > 0)
ORDER BY a.occurred_at DESC, a.id
LIMIT 1000
