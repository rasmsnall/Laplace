-- what the last poll of an sftp or storage flow saw, file by file. a file counts as arrived
-- once it is unchanged between two polls, and only once.
create table file_observations (
    flow text not null,
    key text not null,
    size bigint not null,
    modified timestamptz not null,
    first_seen timestamptz not null,
    counted boolean not null,
    primary key (flow, key)
);

-- set on a flow's first remembered poll. a flow that ran before files were remembered takes
-- what is there on that poll as its starting point, rather than as new arrivals.
alter table flow_state add column files_observed_at timestamptz;
