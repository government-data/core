-- The key of the evaluation's manifest in the tenant's warehouse bucket
-- (decision 0035): what an audit reads for this cut's results.
ALTER TABLE quality.run_summary ADD COLUMN manifest text;
