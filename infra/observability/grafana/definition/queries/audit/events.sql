-- Every event, newest first (at most 1000).
SELECT a.occurred_at AS time, a.event_type, coalesce(u.display_name, u.email, a.service_account) AS who,
  a.action, a.resource, substring(a.resource FROM '^(?:dataset|catalog|pipeline)/([^/]+)') AS dataset,
  a.job_id::text AS job_id, a.ip, a.user_agent, a.email_attempted
FROM public.admin_audit_events a
LEFT JOIN public.admin_users u ON u.id = a.admin_id
WHERE $__timeFilter(a.occurred_at)
  AND ('__all' IN (${event_type:sqlstring}) OR a.event_type IN (${event_type:sqlstring}))
  AND ('__all' IN (${who:sqlstring}) OR a.admin_id::text IN (${who:sqlstring}))
  AND (${resource:sqlstring} = '' OR strpos(a.resource, ${resource:sqlstring}) > 0)
ORDER BY a.occurred_at DESC, a.id
LIMIT 1000
