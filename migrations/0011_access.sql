-- people given a role by email in the settings page
create table members (
    email text primary key,
    role text not null check (role in ('viewer', 'operator', 'editor', 'admin')),
    added_by text not null,
    added_at timestamptz not null default now()
);

-- sso groups (names or entra object ids) that give everyone in them a role
create table group_roles (
    group_name text primary key,
    role text not null check (role in ('viewer', 'operator', 'editor', 'admin')),
    added_by text not null,
    added_at timestamptz not null default now()
);

-- each user's groups as of their last sign-in; tokens can carry hundreds, too many for a cookie
create table user_groups (
    email text primary key,
    groups text[] not null,
    signed_in_at timestamptz not null default now()
);

-- tokens jobs send on /ping and /calls, stored as sha-256 hashes
create table job_tokens (
    id bigint generated always as identity primary key,
    name text not null,
    hash text not null unique,
    created_by text not null,
    created_at timestamptz not null default now(),
    last_used_at timestamptz,
    revoked_at timestamptz
);

-- settings changed in the dashboard; environment variables are the defaults
create table settings (
    key text primary key,
    value text not null,
    changed_by text not null,
    changed_at timestamptz not null default now()
);

create table audit (
    id bigint generated always as identity primary key,
    at timestamptz not null default now(),
    by text not null,
    action text not null,
    detail text not null default ''
);
create index audit_by_time on audit (at desc);
