-- flows a registered flow waits for, sent by the job as ?after=a,b
alter table registered_flows add column after text[] not null default '{}';
