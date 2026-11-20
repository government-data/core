-- A view of personal data records its reason and its shape (decision 0069,
-- point 8): the purpose's key, the reference as given, how many rows the
-- answer showed and the names of the fields it showed, never a value. The
-- view's job goes in `job_id` (0006). Empty for every other event, and for
-- the views recorded before this. The purposes are checked by the server
-- (`contracts::ViewPurpose`), not here: the list only grows, and a retired
-- key must still read.
ALTER TABLE public.admin_audit_events
    ADD COLUMN purpose text,
    ADD COLUMN reference text,
    ADD COLUMN rows_shown integer,
    ADD COLUMN fields_shown text[];
