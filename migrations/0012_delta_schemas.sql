-- the last seen schema of each delta table a flow watches, and its latest change
create table delta_schemas (
    flow text not null,
    table_name text not null,
    fields text[] not null,
    change text,
    changed_at timestamptz,
    primary key (flow, table_name)
);
