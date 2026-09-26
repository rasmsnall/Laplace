-- databricks jobs picked from the workspace are polled like configured ones
alter table registered_flows drop constraint registered_flows_kind_check;
alter table registered_flows add constraint registered_flows_kind_check check (kind in ('heartbeat', 'inbound', 'databricks'));
alter table registered_flows add column job_id bigint;

-- the job's own schedule as the workspace defines it
alter table registered_flows add column cron text;
alter table registered_flows add column timezone text;
