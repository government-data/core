-- A tenant's warehouse made on the main lake by the application (decision
-- 0066 step 11, the user's ruling of 2026-11-13 on the warehouse) whose
-- replication waits for the replica or its keys: the state `made`. A step
-- of making it that failed is kept as `failed` with the step
-- (`warehouse_bucket`, `warehouse_tables`), as the replication's are.
ALTER TABLE public.lake_replication
    DROP CONSTRAINT lake_replication_state_check,
    ADD CONSTRAINT lake_replication_state_check
        CHECK (state IN ('configured', 'no_warehouse', 'no_keys', 'failed', 'made'));
