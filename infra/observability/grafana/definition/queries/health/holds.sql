-- Trino's holds in force: no queued job that uses Trino is handed out
-- until the coordinator's agent says Trino answers again, or the expiry.
SELECT h.machine, h.held_at, h.expires_at
FROM quality.trino_holds h
WHERE h.expires_at > now()
