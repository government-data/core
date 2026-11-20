-- Decision 0074, step 3b: the action `admin:ManageLicense` leaves the catalog.
--
-- Custom policies may still name it. Loading them keeps working (an unknown
-- name grants nothing), but saving one again would be refused as an unknown
-- action. The name is taken out of every statement once, here; a statement
-- left with no action grants nothing and goes too. Patterns (`admin:*`, `*`)
-- are left as they are: they still match other actions.
--
-- The table forces row-level security; as in 0016, the owner sees every
-- tenant's rows only without FORCE, put back in this same transaction.
-- Running it twice changes nothing.
ALTER TABLE public.iam_policies NO FORCE ROW LEVEL SECURITY;
UPDATE public.iam_policies p
   SET statements = COALESCE((
           SELECT jsonb_agg(
                      jsonb_set(s.statement, '{actions}',
                                COALESCE((SELECT jsonb_agg(a.action ORDER BY a.n)
                                            FROM jsonb_array_elements(s.statement -> 'actions')
                                                 WITH ORDINALITY AS a(action, n)
                                           WHERE a.action <> '"admin:ManageLicense"'::jsonb),
                                         '[]'::jsonb))
                      ORDER BY s.n)
             FROM jsonb_array_elements(p.statements) WITH ORDINALITY AS s(statement, n)
            WHERE EXISTS (SELECT 1
                            FROM jsonb_array_elements(s.statement -> 'actions') AS a(action)
                           WHERE a.action <> '"admin:ManageLicense"'::jsonb)),
           '[]'::jsonb),
       updated_at = now()
 WHERE p.statements @> '[{"actions": ["admin:ManageLicense"]}]'::jsonb;
ALTER TABLE public.iam_policies FORCE ROW LEVEL SECURITY;
