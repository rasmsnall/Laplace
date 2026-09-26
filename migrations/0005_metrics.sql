-- numbers a run reported about itself, e.g. {"rows": 1204331, "seconds": 812}
alter table events add column metrics jsonb;

-- set when the latest run's numbers were far from the usual ones, cleared by the next normal run
alter table flow_state add column anomaly text;
