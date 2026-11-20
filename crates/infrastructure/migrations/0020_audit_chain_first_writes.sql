-- Decision 0070, point 7, made safe for concurrent first writes.
--
-- Two transactions sealing a tenant's first audit rows at once could both
-- find no head. The one whose check ran after the other committed then saw
-- that row and refused ("has audit rows but no head"), and the audited
-- action failed with it: a race on the first actions of a new installation.
--
-- Opening a tenant's chain is now serialised by a transaction-scoped
-- advisory lock, and the head is looked up again once the lock is held, so a
-- transaction that waited finds the head the other one opened. The rule of
-- point 7 stands: a tenant with rows and no head is still refused.
CREATE OR REPLACE FUNCTION public.audit_chain_seal() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    head record;
BEGIN
    SELECT seq, hash INTO head FROM public.audit_chain_heads
     WHERE tenant_id = NEW.tenant_id FOR UPDATE;
    IF NOT FOUND THEN
        PERFORM pg_advisory_xact_lock(
            hashtextextended('audit_chain_open:' || NEW.tenant_id::text, 0));
        SELECT seq, hash INTO head FROM public.audit_chain_heads
         WHERE tenant_id = NEW.tenant_id FOR UPDATE;
    END IF;
    IF NOT FOUND THEN
        IF EXISTS (SELECT 1 FROM public.admin_audit_events WHERE tenant_id = NEW.tenant_id) THEN
            RAISE EXCEPTION 'audit chain: tenant % has audit rows but no head', NEW.tenant_id;
        END IF;
        INSERT INTO public.audit_chain_heads (tenant_id, seq, hash, adopted_seq, adopted_at)
        VALUES (NEW.tenant_id, 0, public.audit_chain_genesis(NEW.tenant_id), 0, now())
        ON CONFLICT (tenant_id) DO NOTHING;
        SELECT seq, hash INTO head FROM public.audit_chain_heads
         WHERE tenant_id = NEW.tenant_id FOR UPDATE;
    END IF;
    NEW.chain_seq := head.seq + 1;
    NEW.chain_version := 1;
    NEW.chain_hash := sha256(public.audit_row_v1(head.hash, NEW.chain_seq, NEW));
    UPDATE public.audit_chain_heads SET seq = NEW.chain_seq, hash = NEW.chain_hash
     WHERE tenant_id = NEW.tenant_id;
    RETURN NEW;
END
$$;
