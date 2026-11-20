-- What the charts group a rule's counts by (decision 0031): its field,
-- pattern, dimension and severity as the worker compiled it. Empty in the
-- rows an older worker reported, which sends counts only.
ALTER TABLE quality.rule_results
    ADD COLUMN field text,
    ADD COLUMN pattern text,
    ADD COLUMN dimension text,
    ADD COLUMN severity text;
