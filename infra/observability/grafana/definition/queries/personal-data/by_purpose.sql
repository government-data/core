-- Views per purpose (decision 0069); a view audited before reveals asked
-- for one has none, '' here, and reads as not recorded.
SELECT coalesce(a.purpose, '') AS purpose, count(*) AS views
FROM public.admin_audit_events a
WHERE $__timeFilter(a.occurred_at)
  AND a.event_type = 'records_viewed'
  AND ('__all' IN (${who:sqlstring}) OR a.admin_id::text IN (${who:sqlstring}))
  AND (${resource:sqlstring} = '' OR strpos(a.resource, ${resource:sqlstring}) > 0)
GROUP BY 1
ORDER BY 2 DESC, 1
