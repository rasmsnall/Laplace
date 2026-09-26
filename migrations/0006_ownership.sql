-- someone has seen the problem; cleared when the flow is ok again
alter table flow_state add column ack_note text;
alter table flow_state add column ack_by text;
alter table flow_state add column ack_at timestamptz;

-- no alerts until then, for maintenance or a known outage
alter table flow_state add column silenced_until timestamptz;
alter table flow_state add column silence_note text;

alter table registered_flows add column owner text;
