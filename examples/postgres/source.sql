CREATE SCHEMA dm_source;
CREATE TABLE dm_source.orders (
  order_id integer PRIMARY KEY,
  cents bigint NOT NULL,
  note text,
  active boolean NOT NULL
);
INSERT INTO dm_source.orders VALUES
  (1, 12500, 'Synthetic example order', true),
  (2, -750, NULL, false),
  (3, 0, 'Unicode example: 雪🦀', true);
