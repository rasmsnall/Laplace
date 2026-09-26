-- a warning is set by the latest check (an expiring certificate) and replaced on every poll
alter table flow_state add column check_warning text;

-- trust on first use: the first key seen is trusted, a different one is held as pending until confirmed
create table host_keys (
    flow text primary key,
    fingerprint text not null,
    pending text,
    seen_at timestamptz not null default now()
);
