-- The system's awake time, in seconds since boot without the time spent
-- asleep, at the host's last heartbeat. A host's heartbeat timer only runs
-- while the system is awake, so staleness is judged on this clock; it is null
-- where the platform has no such clock.
ALTER TABLE local_host_status ADD COLUMN heartbeat_awake INTEGER;
