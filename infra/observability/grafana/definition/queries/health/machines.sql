-- Worker machines that may work, per pool, as registered: approved or
-- draining, and how many of them hold a certificate still valid. Not
-- whether they are alive.
SELECT
  coalesce(m.pool, '') AS pool,
  count(*) FILTER (WHERE m.state = 'approved') AS approved,
  count(*) FILTER (WHERE m.state = 'draining') AS draining,
  count(*) FILTER (WHERE m.cert_not_after > now()) AS valid_certificate
FROM public.machines m
WHERE m.role = 'worker' AND m.state IN ('approved', 'draining')
GROUP BY m.pool
ORDER BY 1
