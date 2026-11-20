-- Machines whose certificate ends within 14 days, or has ended.
SELECT m.name, m.role, coalesce(m.pool, '') AS pool, m.state, m.cert_not_after
FROM public.machines m
WHERE m.state <> 'retired' AND m.cert_not_after < now() + interval '14 days'
ORDER BY m.cert_not_after
