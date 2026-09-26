-- one row per flow: everything the status of a flow is computed from
create table flow_state (
    flow text primary key,
    last_ok timestamptz,
    running_since timestamptz,
    problem text,
    alerted_state text not null default 'pending'
);

create table events (
    id bigint generated always as identity primary key,
    flow text not null,
    at timestamptz not null,
    outcome text not null check (outcome in ('start', 'ok', 'fail')),
    detail text not null default '',
    millis integer
);
create index events_by_flow on events (flow, at desc);
create index events_by_age on events (at);

-- inbound api traffic, counted per minute so storage does not grow with request volume
create table calls (
    flow text not null,
    minute timestamptz not null,
    ok integer not null default 0,
    client_errors integer not null default 0,
    server_errors integer not null default 0,
    total_millis bigint not null default 0,
    primary key (flow, minute)
);

-- due work, claimed with `for update skip locked` so every job runs on exactly one replica
create table jobs (
    name text primary key,
    next_run_at timestamptz not null default now()
);

create table export_watermarks (
    name text primary key,
    watermark bigint not null
);
