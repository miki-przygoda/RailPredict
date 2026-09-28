-- Extend delay_history's quarterly partitions past 2026-07-01, and add a DEFAULT
-- partition so a missing range can never reject inserts again.
--
-- Migration 006 created partitions only up to 2026 Q2. From 2026-07-01 every
-- INSERT failed with "no partition of relation delay_history found for row"
-- (SQLSTATE 23514), so production stopped recording delay history and the
-- db_* integration tests, which insert NOW(), started failing.
--
-- The DEFAULT partition catches any row outside the named ranges. Caveat: a new
-- range partition cannot be created while the default holds rows in that range;
-- move those rows out first (DETACH the default, create the range, re-insert,
-- re-ATTACH). Add the next quarters well before 2029-01-01 to keep the default
-- empty.
--
-- IF NOT EXISTS keeps this safe on databases where an operator already added
-- some of these partitions by hand. The indexes from 006/007 propagate to each
-- new partition automatically (PG14+).
--
-- DO NOT add explicit BEGIN/COMMIT here -- sqlx wraps every migration in its
-- own transaction (see 006).

CREATE TABLE IF NOT EXISTS delay_history_2026_q3 PARTITION OF delay_history
    FOR VALUES FROM ('2026-07-01') TO ('2026-10-01');
CREATE TABLE IF NOT EXISTS delay_history_2026_q4 PARTITION OF delay_history
    FOR VALUES FROM ('2026-10-01') TO ('2027-01-01');
CREATE TABLE IF NOT EXISTS delay_history_2027_q1 PARTITION OF delay_history
    FOR VALUES FROM ('2027-01-01') TO ('2027-04-01');
CREATE TABLE IF NOT EXISTS delay_history_2027_q2 PARTITION OF delay_history
    FOR VALUES FROM ('2027-04-01') TO ('2027-07-01');
CREATE TABLE IF NOT EXISTS delay_history_2027_q3 PARTITION OF delay_history
    FOR VALUES FROM ('2027-07-01') TO ('2027-10-01');
CREATE TABLE IF NOT EXISTS delay_history_2027_q4 PARTITION OF delay_history
    FOR VALUES FROM ('2027-10-01') TO ('2028-01-01');
CREATE TABLE IF NOT EXISTS delay_history_2028_q1 PARTITION OF delay_history
    FOR VALUES FROM ('2028-01-01') TO ('2028-04-01');
CREATE TABLE IF NOT EXISTS delay_history_2028_q2 PARTITION OF delay_history
    FOR VALUES FROM ('2028-04-01') TO ('2028-07-01');
CREATE TABLE IF NOT EXISTS delay_history_2028_q3 PARTITION OF delay_history
    FOR VALUES FROM ('2028-07-01') TO ('2028-10-01');
CREATE TABLE IF NOT EXISTS delay_history_2028_q4 PARTITION OF delay_history
    FOR VALUES FROM ('2028-10-01') TO ('2029-01-01');

CREATE TABLE IF NOT EXISTS delay_history_default PARTITION OF delay_history DEFAULT;
