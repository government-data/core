-- A renewed certificate (decision 0066, point 14) is the machine's next one
-- until the machine first presents it: then it is the one in use and the
-- next is cleared. Until then both are accepted, so a machine that renewed
-- and lost the answer still works with the one it has, and renews again.
ALTER TABLE public.machines ADD COLUMN cert_serial_next text CHECK (cert_serial_next ~ '^[0-9a-f]{1,64}$');
ALTER TABLE public.machines ADD COLUMN cert_not_after_next timestamp with time zone;
