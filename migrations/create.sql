-- note: this schema is only used for postgres_logger
CREATE SCHEMA lite_rpc;

CREATE TABLE lite_rpc.Txs (
  id SERIAL NOT NULL PRIMARY KEY,
  signature VARCHAR(88) NOT NULL,
  recent_slot BIGINT NOT NULL,
  forwarded_slot BIGINT NOT NULL,
  forwarded_local_time TIMESTAMP WITH TIME ZONE NOT NULL,
  processed_slot BIGINT,
  cu_consumed BIGINT,
  cu_requested BIGINT,
  cu_price BIGINT,
  quic_response SMALLINT
);


CREATE TABLE lite_rpc.Blocks (
  slot BIGINT NOT NULL PRIMARY KEY,
  leader_id BIGINT NOT NULL,
  parent_slot BIGINT NOT NULL,
  cluster_time TIMESTAMP WITH TIME ZONE NOT NULL,
  local_time TIMESTAMP WITH TIME ZONE
);

CREATE TABLE lite_rpc.AccountAddrs (
  id SERIAL PRIMARY KEY,
  addr VARCHAR(45) NOT NULL
);

CREATE TABLE lite_rpc.StakeInfoAccounts (
  pubkey VARCHAR(44) NOT NULL PRIMARY KEY,
  start_count_date BIGINT NOT NULL,
  stop_count_date BIGINT,
  spent_charges BIGINT NOT NULL,
  saved_charges BIGINT NOT NULL,
  last_charge_date BIGINT,
  restorable_charges BIGINT NOT NULL,
  amount BIGINT NOT NULL,
  stake_quantity SMALLINT NOT NULL
);