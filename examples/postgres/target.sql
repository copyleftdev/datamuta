CREATE SCHEMA dm_target;
CREATE TABLE dm_target.purchases (
  order_key bigint PRIMARY KEY,
  value bigint NOT NULL,
  memo text,
  enabled boolean NOT NULL
);
CREATE TABLE dm_target.stream_receipts(id bigint PRIMARY KEY, hash text NOT NULL);
CREATE TABLE dm_target.stream_cursor(
  id integer PRIMARY KEY, plan text NOT NULL, next bigint NOT NULL, certificate text NOT NULL
);
INSERT INTO dm_target.stream_cursor VALUES(1, '', 0, '');
