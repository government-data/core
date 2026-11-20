-- The lake's maintenance per dataset (an empty dataset is the whole
-- warehouse): the last one done, the last one failed, and those pending.
SELECT
  coalesce(j.dataset, '') AS dataset,
  max(j.finished_at) FILTER (WHERE j.state = 'done') AS last_done,
  max(j.finished_at) FILTER (WHERE j.state = 'failed') AS last_failed,
  count(*) FILTER (WHERE j.state IN ('queued', 'running')) AS pending
FROM quality.jobs j
WHERE j.kind = 'maintain'
GROUP BY j.dataset
ORDER BY 1
