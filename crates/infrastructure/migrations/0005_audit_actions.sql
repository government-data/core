-- The audit log grows from administration to quality (decision 0026): an
-- event done under an IAM action records the action, the resource it was
-- done on, and the service account it came with. Empty for the events that
-- came before (sign-in, accounts).
ALTER TABLE public.admin_audit_events
    ADD COLUMN action text,
    ADD COLUMN resource text,
    ADD COLUMN service_account text;
