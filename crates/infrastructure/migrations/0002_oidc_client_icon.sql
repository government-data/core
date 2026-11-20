-- The icon of an application that signs in with this installation, for the
-- avatar's «Aplicaciones» (ADR 0016): an https address, usually its own.
ALTER TABLE public.oidc_clients ADD COLUMN icon_url text;
