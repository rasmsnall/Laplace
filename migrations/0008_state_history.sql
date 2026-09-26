-- every change of a flow's state, so uptime can be measured over any period
create table state_changes (
    id bigint generated always as identity primary key,
    flow text not null,
    at timestamptz not null default now(),
    state text not null,
    detail text not null default ''
);
create index state_changes_by_flow on state_changes (flow, at);

-- the last state written to state_changes, kept apart from what was alerted
alter table flow_state add column recorded_state text;
