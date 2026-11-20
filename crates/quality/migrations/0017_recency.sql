-- The update recency (decision 0031): the `updated` field's records by age
-- at the cut, after the cut, up to 30, 90, 180, 365 and 730 days, then
-- older; NULL for every other field.
ALTER TABLE quality.field_quality ADD COLUMN recency bigint[];
