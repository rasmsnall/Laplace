-- what each databricks job cost per day, from the billing system tables, at list price
create table job_costs (
    job_id bigint not null,
    day date not null,
    dbus double precision not null,
    cost double precision not null,
    currency text not null,
    primary key (job_id, day)
);

-- set when yesterday cost far more than usual for the job
alter table flow_state add column cost_warning text;
