-- A schedule's trigger (decision 0035, point 1): once, every N days, every N
-- weeks on some days, or some months on some days or the nth weekday, with
-- repetition, a random delay and expiry (schedule_core::ScheduleTrigger).
-- With one the schedule fires by it and `cron` is empty; without, by its
-- cron, as before. A schedule that will not fire again keeps no next fire.
ALTER TABLE quality.schedules ADD COLUMN trigger jsonb;
