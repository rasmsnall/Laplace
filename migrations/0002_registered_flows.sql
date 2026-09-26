-- flows created by a job reporting under a new name, so connecting a job needs no config change
create table registered_flows (
    id text primary key,
    kind text not null check (kind in ('heartbeat', 'inbound')),
    every_seconds bigint not null,
    created_at timestamptz not null default now()
);
