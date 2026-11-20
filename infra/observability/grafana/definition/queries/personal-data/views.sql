-- Every view, newest first (at most 1000), with its reason and shape
-- (decision 0069): '' as the purpose of a view audited before reveals asked
-- for one, which reads as not recorded; never a value.
SELECT a.occurred_at AS time, coalesce(u.display_name, u.email, a.service_account) AS who,
  split_part(a.resource, '/', 1) AS kind, coalesce(a.purpose, '') AS purpose, a.reference,
  a.rows_shown, array_to_string(a.fields_shown, ', ') AS fields_shown, a.resource,
  a.job_id::text AS job_id
FROM public.admin_audit_events a
LEFT JOIN public.admin_users u ON u.id = a.admin_id
WHERE $__timeFilter(a.occurred_at)
  AND a.event_type = 'records_viewed'
  AND ('__all' IN (${who:sqlstring}) OR a.admin_id::text IN (${who:sqlstring}))
  AND (${resource:sqlstring} = '' OR strpos(a.resource, ${resource:sqlstring}) > 0)
ORDER BY a.occurred_at DESC, a.id
LIMIT 1000
