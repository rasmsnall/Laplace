-- how long after its deadline a registered flow may still succeed, sent as ?grace=45m
alter table registered_flows add column grace_seconds bigint;
