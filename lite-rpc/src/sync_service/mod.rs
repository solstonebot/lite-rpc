use std::str::FromStr;

use serde::Deserialize;
use solana_rpc_client_api::{config::RpcProgramAccountsConfig, filter::RpcFilterType};
use solana_sdk::pubkey::Pubkey;
use tokio_cron_scheduler::{Job, JobScheduler, JobSchedulerError};
use tokio_postgres::types::ToSql;

use crate::postgres_logger::PostgresSession;

#[derive(Deserialize, Debug)]
pub struct StakeInfo {
  pub discriminator: u64,
  pub start_count_date: i64,
  pub stop_count_date: Option<i64>,
  pub spent_charges: i64,
  pub saved_charges: i64,
  pub last_charge_date: Option<i64>,
  pub restorable_charges: i64,
  pub amount: i64,
  pub stake_quantity: i16,
}

pub struct SyncService {}

impl SyncService {
  pub async fn start(postgres_session: PostgresSession) -> Result<(), JobSchedulerError> {
    let sched = JobScheduler::new().await?;

    sched.add(
      Job::new_async("1/1 * * * * *", move |_uuid, _l| {
        let postgres = postgres_session.client.clone();

        Box::pin(async move {
          let program_id: Pubkey = Pubkey::from_str("7zypKjkAJGAqhsQgJzzCr6iPKjEvUjBMmMJiimgdHc8n").unwrap();
          let connection = solana_rpc_client::rpc_client::RpcClient::new("http://localhost:8899".to_string());

          // println!("fetch");
          let accounts = connection.get_program_accounts_with_config(&program_id, RpcProgramAccountsConfig {
            filters: Some(vec![
              RpcFilterType::DataSize(88),
            ]),
            ..RpcProgramAccountsConfig::default()
          }).unwrap();

          const NB_ARGUMENTS: usize = 9;

          let mut args: Vec<&(dyn ToSql + Sync)> = Vec::with_capacity(NB_ARGUMENTS * accounts.len());

          let mut accounts_data_vec: Vec<(String, StakeInfo)> = vec![];
          for (pubkey, acc) in accounts {
            let account_data = acc.deserialize_data::<StakeInfo>().unwrap();

            // println!("{:?}", account_data);

            accounts_data_vec.push((pubkey.to_string(), account_data));
          }

          for (pubkey, acc) in accounts_data_vec.iter() {
            let StakeInfo {
              discriminator: _,
              start_count_date,
              stop_count_date,
              spent_charges,
              saved_charges,
              last_charge_date,
              restorable_charges,
              amount,
              stake_quantity
            } = acc;

            args.push(pubkey);
            args.push(start_count_date);
            args.push(stop_count_date);
            args.push(spent_charges);
            args.push(saved_charges);
            args.push(last_charge_date);
            args.push(restorable_charges);
            args.push(amount);
            args.push(stake_quantity);
          }

          let values = PostgresSession::values_vecvec(NB_ARGUMENTS, accounts_data_vec.len(), &[]);
          let statement = format!(
              r#"
                  INSERT INTO lite_rpc.StakeInfoAccounts
                  (pubkey, start_count_date, stop_count_date, spent_charges, saved_charges, last_charge_date, restorable_charges, amount, stake_quantity)
                  VALUES {}
                  ON CONFLICT (pubkey) DO UPDATE SET
                  start_count_date = EXCLUDED.start_count_date,
                  stop_count_date = EXCLUDED.stop_count_date,
                  spent_charges = EXCLUDED.spent_charges,
                  saved_charges = EXCLUDED.saved_charges,
                  last_charge_date = EXCLUDED.last_charge_date,
                  restorable_charges = EXCLUDED.restorable_charges,
                  amount = EXCLUDED.amount,
                  stake_quantity = EXCLUDED.stake_quantity
              "#,
              values
          );

          let _ = postgres.execute(&statement, &args).await;
        })
      })?
    ).await?;

    sched.start().await?;

    Ok(())
  }
}