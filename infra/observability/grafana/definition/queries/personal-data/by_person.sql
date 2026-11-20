-- Views per person (the 20 with most).
SELECT coalesce(u.display_name, u.email, a.service_account, '—') AS who, count(*) AS views
FROM public.admin_audit_events a
LEFT JOIN public.admin_users u ON u.id = a.admin_id
WHERE $__timeFilter(a.occurred_at)
  AND a.event_type = 'records_viewed'
  AND ('__all' IN (${who:sqlstring}) OR a.admin_id::text IN (${who:sqlstring}))
  AND (${resource:sqlstring} = '' OR strpos(a.resource, ${resource:sqlstring}) > 0)
GROUP BY 1
ORDER BY 2 DESC, 1
LIMIT 20
