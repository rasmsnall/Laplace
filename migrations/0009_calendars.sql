-- business-day calendars a registered flow's schedule follows, e.g. {se,fi}
alter table registered_flows add column calendar text[] not null default '{}';
