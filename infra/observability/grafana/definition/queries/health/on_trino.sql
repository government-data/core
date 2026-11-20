-- Jobs that count as using Trino (quality.on_trino, as the hold counts
-- them) running now.
SELECT count(*) AS on_trino
FROM quality.jobs j
WHERE j.state = 'running' AND j.lease_until > now() AND quality.on_trino(j.kind, j.uses_trino)
