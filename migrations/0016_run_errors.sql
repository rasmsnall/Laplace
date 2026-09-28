-- the end of a failed run's output, secrets masked, so the error can be read without the server
alter table events add column error text;
