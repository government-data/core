-- Credentials issued to a job are audited with the job they were for and
-- the machine they went to (decision 0030, point 3): the machine in
-- `service_account` (its client id), the job here. Empty for every other
-- event.
ALTER TABLE public.admin_audit_events ADD COLUMN job_id uuid;
