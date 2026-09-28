-- when each row was written, as opposed to when the run happened. the export to delta only
-- takes rows written a while ago, so a row whose transaction commits late is not skipped.
alter table events add column recorded_at timestamptz not null default clock_timestamp();
alter table state_changes add column recorded_at timestamptz not null default clock_timestamp();
