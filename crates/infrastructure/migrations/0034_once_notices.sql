-- Notices the administrators get once per installation (decision 0090,
-- ruling 78: the flip to Trino's results): the notice's key and when it
-- went out. An expansion: one new table.
CREATE TABLE public.once_notices (
    key text PRIMARY KEY,
    sent_at timestamp with time zone NOT NULL DEFAULT now()
);
