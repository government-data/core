-- How long the oldest job ready to take has waited; 0 with none ready.
SELECT coalesce(extract(epoch FROM now() - min(j.not_before)), 0) AS oldest_ready
FROM quality.jobs j
WHERE j.state = 'queued' AND j.not_before <= now()
