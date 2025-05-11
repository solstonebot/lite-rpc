// This class will manage the lifecycle for a transaction
// It will send, replay if necessary and confirm by listening to blocks

use std::{str::FromStr, sync::Arc, time::Duration};

use crate::{
    tpu_utils::tpu_service::TpuService,
    transaction_replayer::{TransactionReplay, TransactionReplayer, MESSAGES_IN_REPLAY_QUEUE},
    tx_sender::TxSender,
};
use anyhow::bail;
use chrono::{DateTime, Local};
use prometheus::{histogram_opts, register_histogram, Histogram};
use solana_lite_rpc_core::{
    solana_utils::SerializableTransaction, structures::transaction_sent_info::SentTransactionInfo,
    types::SlotStream,
};
use solana_lite_rpc_core::{
    stores::block_information_store::{BlockInformation, BlockInformationStore},
    structures::notifications::NotificationSender,
    AnyhowJoinHandle,
};
use solana_sdk::{
    compute_budget::{self, ComputeBudgetInstruction}, pubkey::Pubkey, transaction::VersionedTransaction
};
use tokio::{
    sync::mpsc::{self, Sender, UnboundedSender},
    time::Instant,
};
use tokio_postgres::{types::ToSql, Client};

lazy_static::lazy_static! {
    static ref PRIORITY_FEES_HISTOGRAM: Histogram = register_histogram!(histogram_opts!(
        "literpc_txs_priority_fee",
        "Priority fees of transactions sent by lite-rpc",
    ))
    .unwrap();
}

#[derive(Clone)]
pub struct TransactionServiceBuilder {
    tx_sender: TxSender,
    tx_replayer: TransactionReplayer,
    tpu_service: TpuService,
    max_nb_txs_in_queue: usize,
    postgres: Arc<Client>,
}

impl TransactionServiceBuilder {
    pub fn new(
        tx_sender: TxSender,
        tx_replayer: TransactionReplayer,
        tpu_service: TpuService,
        max_nb_txs_in_queue: usize,
        postgres: Arc<Client>,
    ) -> Self {
        Self {
            tx_sender,
            tx_replayer,
            tpu_service,
            max_nb_txs_in_queue,
            postgres,
        }
    }

    pub fn start(
        self,
        notifier: Option<NotificationSender>,
        block_information_store: BlockInformationStore,
        max_retries: usize,
        slot_notifications: SlotStream,
    ) -> (TransactionService, AnyhowJoinHandle) {
        let (transaction_channel, tx_recv) = mpsc::channel(self.max_nb_txs_in_queue);
        let (replay_channel, replay_reciever) = tokio::sync::mpsc::unbounded_channel();

        let jh_services: AnyhowJoinHandle = {
            let tx_sender = self.tx_sender.clone();
            let tx_replayer = self.tx_replayer.clone();
            let tpu_service = self.tpu_service.clone();
            let replay_channel_task = replay_channel.clone();

            tokio::spawn(async move {
                let tpu_service_fx = tpu_service.start(slot_notifications);

                let tx_sender_jh = tx_sender.clone().execute(tx_recv, notifier.clone());

                let replay_service =
                    tx_replayer.start_service(replay_channel_task, replay_reciever);

                tokio::select! {
                    res = tpu_service_fx => {
                        bail!("Tpu Service {res:?}")
                    },
                    res = tx_sender_jh => {
                        bail!("Tx Sender {res:?}")
                    },
                    res = replay_service => {
                        bail!("Replay Service {res:?}")
                    },
                }
            })
        };

        (
            TransactionService {
                transaction_channel,
                replay_channel,
                block_information_store,
                max_retries,
                replay_offset: self.tx_replayer.retry_offset,
                postgres: self.postgres,
                global: GlobalConfig {
                    charges_per_day: 4,
                    charges_per_tx: 2,
                    restorable_charges: 2,
                },
            },
            jh_services,
        )
    }
}

#[derive(Debug)]
pub struct StakeInfoAccont {
    pub payer: String,
    pub start_count_date: i64,
    pub stop_count_date: Option<i64>,
    pub spent_charges: i64,
    pub saved_charges: i64,
    pub last_charge_date: Option<i64>,
    pub restorable_charges: i64,
    pub amount: i64,
    pub stake_quantity: i16,
}

#[derive(Clone)]
pub struct GlobalConfig {
    charges_per_day: i64,
    charges_per_tx: i64,
    restorable_charges: i64,
}

#[derive(Clone)]
pub struct TransactionService {
    pub transaction_channel: Sender<SentTransactionInfo>,
    pub replay_channel: UnboundedSender<TransactionReplay>,
    pub block_information_store: BlockInformationStore,
    pub max_retries: usize,
    pub replay_offset: Duration,
    pub postgres: Arc<Client>,
    pub global: GlobalConfig,
}

impl TransactionService {
    pub async fn send_transaction(
        &self,
        tx: VersionedTransaction,
        max_retries: Option<u16>,
    ) -> anyhow::Result<String> {
        let raw_tx = bincode::serialize(&tx)?;
        self.send_wire_transaction(raw_tx, max_retries).await
    }

    pub async fn send_wire_transaction(
        &self,
        raw_tx: Vec<u8>,
        max_retries: Option<u16>,
    ) -> anyhow::Result<String> {
        println!("TX");
        let tx = match bincode::deserialize::<VersionedTransaction>(&raw_tx) {
            Ok(tx) => tx,
            Err(err) => {
                bail!(err.to_string());
            }
        };

        let (payer, _) = Pubkey::find_program_address(&[
            b"stake",
            &tx.message.static_account_keys()[0].to_bytes(),
        ], &Pubkey::from_str("7zypKjkAJGAqhsQgJzzCr6iPKjEvUjBMmMJiimgdHc8n").unwrap());

        println!("{:?}", payer);

        let mut args: Vec<&(dyn ToSql + Sync)> = Vec::with_capacity(1);

        let pubkey = &payer.clone().to_string();
        args.push(pubkey);

        let statement = format!(
            r#"
                SELECT * FROM lite_rpc.StakeInfoAccounts WHERE pubkey = $1
            "#,
        );

        match self.postgres.query(&statement, &args).await
            {
                Ok(rows) => {
                    if rows.len() == 0 {
                        bail!("Stake not exists!");
                    }

                    let stake_info_row = &rows[0];

                    let mut stake_info = StakeInfoAccont {
                        payer: stake_info_row.get("pubkey"),
                        start_count_date: stake_info_row.get("start_count_date"),
                        stop_count_date: stake_info_row.get("stop_count_date"),
                        spent_charges: stake_info_row.get("spent_charges"),
                        saved_charges: stake_info_row.get("saved_charges"),
                        last_charge_date: stake_info_row.get("last_charge_date"),
                        restorable_charges: stake_info_row.get("restorable_charges"),
                        amount: stake_info_row.get("amount"),
                        stake_quantity: stake_info_row.get("stake_quantity"),
                    };

                    let dt = Local::now();
                    let now = DateTime::<Local>::from_naive_utc_and_offset(dt.naive_utc(), dt.offset().clone()).timestamp();
                    const DAY_UNIX: i64 = 24 * 60 * 60;
                    let stake_rewards = ((now - stake_info.start_count_date) / DAY_UNIX) * self.global.charges_per_day;
                    stake_info.spent_charges = stake_info.spent_charges - stake_info.restorable_charges;
                
                    if stake_info.last_charge_date.is_some() && stake_info.last_charge_date.unwrap() - now < DAY_UNIX {
                        stake_info.spent_charges = stake_info.spent_charges + self.global.restorable_charges;
                    } else if stake_info.stop_count_date.is_none() {
                        stake_info.last_charge_date = Some(now);
                        stake_info.restorable_charges = stake_info.restorable_charges.checked_add(self.global.restorable_charges).unwrap();
                    }

                    let available_charges = stake_info.saved_charges + stake_rewards - stake_info.spent_charges;

                    if available_charges <= self.global.charges_per_tx {
                        bail!("Insufficient Quote Points: {}, required: {}", available_charges, self.global.charges_per_tx);
                    }

                    let mut args_update: Vec<&(dyn ToSql + Sync)> = Vec::with_capacity(4);

                    args_update.push(&stake_info.spent_charges);
                    args_update.push(&stake_info.last_charge_date);
                    args_update.push(&stake_info.restorable_charges);
                    args_update.push(pubkey);

                    let statement_update = format!(
                        r#"
                            UPDATE lite_rpc.StakeInfoAccounts
                            SET
                            spent_charges = $1
                            last_charge_date = $2
                            restorable_charges = $3
                            WHERE pubkey = $4
                        "#,
                    );

                    self.postgres.execute(&statement_update, &args).await?;
                }
                Err(err) => {
                    bail!("DB error {}", err);
                }
            }

        let signature = tx.signatures[0];

        let Some(BlockInformation {
            slot,
            last_valid_blockheight,
            ..
        }) = self
            .block_information_store
            .get_block_info(tx.get_recent_blockhash())
        else {
            bail!("Blockhash not found in block store".to_string());
        };

        if self.block_information_store.get_last_blockheight() > last_valid_blockheight {
            bail!("Blockhash is expired");
        }

        let prioritization_fee = {
            let mut prioritization_fee = 0;
            for ix in tx.message.instructions() {
                if ix
                    .program_id(tx.message.static_account_keys())
                    .eq(&compute_budget::id())
                {
                    let ix_which = solana_sdk::borsh1::try_from_slice_unchecked::<
                        ComputeBudgetInstruction,
                    >(ix.data.as_slice());
                    if let Ok(ComputeBudgetInstruction::SetComputeUnitPrice(fees)) = ix_which {
                        prioritization_fee = fees;
                    }
                }
            }
            prioritization_fee
        };

        PRIORITY_FEES_HISTOGRAM.observe(prioritization_fee as f64);

        let max_replay = max_retries.map_or(self.max_retries, |x| x as usize);
        let transaction_info = SentTransactionInfo {
            signature,
            last_valid_block_height: last_valid_blockheight,
            slot,
            transaction: Arc::new(raw_tx),
            prioritization_fee,
        };
        if let Err(e) = self
            .transaction_channel
            .send(transaction_info.clone())
            .await
        {
            bail!(
                "Internal error sending transaction on send channel error {}",
                e
            );
        }
        let replay_at = Instant::now() + self.replay_offset;
        // ignore error for replay service
        if self
            .replay_channel
            .send(TransactionReplay {
                transaction: transaction_info,
                replay_count: 0,
                max_replay,
                replay_at,
            })
            .is_ok()
        {
            MESSAGES_IN_REPLAY_QUEUE.inc();
        }
        Ok(signature.to_string())
    }
}

mod test {}
