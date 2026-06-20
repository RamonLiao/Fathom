//! Postgres output sink. `emit` maps to a row and `try_send`s onto a bounded
//! channel (sync, non-blocking); a background writer task owns the pool and does
//! the inserts. Channel Full / writer-dead → loud Err (Rule 12), never a drop.

use anyhow::{anyhow, Context, Result};
use sqlx::postgres::PgPoolOptions;
use std::time::Duration;
use tokio::sync::mpsc;

use crate::row::{to_row, Row};
use crate::sink::{DecodedEvent, EventId, Sink};

pub const CHANNEL_CAPACITY: usize = 1024;

pub fn channel() -> (mpsc::Sender<Row>, mpsc::Receiver<Row>) {
    mpsc::channel(CHANNEL_CAPACITY)
}

pub struct PostgresSink {
    tx: mpsc::Sender<Row>,
}

impl PostgresSink {
    pub fn new(tx: mpsc::Sender<Row>) -> Self {
        Self { tx }
    }
}

impl Sink for PostgresSink {
    fn emit(&self, id: &EventId, checkpoint_seq: u64, ev: &DecodedEvent) -> Result<()> {
        let row = to_row(id, checkpoint_seq, ev);
        self.tx.try_send(row).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => anyhow!(
                "postgres writer cannot keep up (channel full, cap={CHANNEL_CAPACITY}) — DB too slow"
            ),
            mpsc::error::TrySendError::Closed(_) => anyhow!("postgres writer task has died"),
        })
    }
}

/// Connect, set an acquire timeout (a hung DB becomes a loud error, not an
/// unbounded wait), and run migrations. Fatal on failure.
pub async fn connect_pool(database_url: &str) -> Result<sqlx::PgPool> {
    let pool = PgPoolOptions::new()
        .acquire_timeout(Duration::from_secs(10))
        .connect(database_url)
        .await
        .context("connect to DATABASE_URL")?;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("run migrations")?;
    Ok(pool)
}

/// Drain the channel and insert each row. Returns when the channel closes (all
/// senders dropped) after draining everything. An insert error is fatal.
pub async fn run_writer(mut rx: mpsc::Receiver<Row>, pool: sqlx::PgPool) -> Result<()> {
    while let Some(row) = rx.recv().await {
        match row {
            Row::Prices(r) => {
                sqlx::query(
                    "INSERT INTO prices_update \
                     (tx_digest,event_index,checkpoint_seq,oracle_id,spot,forward,ts_chain_ms) \
                     VALUES ($1,$2,$3,$4,$5::numeric,$6::numeric,$7::numeric) \
                     ON CONFLICT (tx_digest,event_index) DO NOTHING",
                )
                .bind(&r.tx_digest).bind(r.event_index).bind(r.checkpoint_seq)
                .bind(&r.oracle_id).bind(&r.spot).bind(&r.forward).bind(&r.ts_chain_ms)
                .execute(&pool).await.context("insert prices_update")?;
            }
            Row::Svi(r) => {
                sqlx::query(
                    "INSERT INTO svi_update \
                     (tx_digest,event_index,checkpoint_seq,oracle_id,a,b,sigma,rho,m,ts_chain_ms,sanity_forward,sanity,sanity_reasons) \
                     VALUES ($1,$2,$3,$4,$5::numeric,$6::numeric,$7::numeric,$8::numeric,$9::numeric,$10::numeric,$11::numeric,$12,$13) \
                     ON CONFLICT (tx_digest,event_index) DO NOTHING",
                )
                .bind(&r.tx_digest).bind(r.event_index).bind(r.checkpoint_seq).bind(&r.oracle_id)
                .bind(&r.a).bind(&r.b).bind(&r.sigma).bind(&r.rho).bind(&r.m).bind(&r.ts_chain_ms)
                .bind(r.sanity_forward.as_deref()).bind(&r.sanity).bind(r.sanity_reasons.as_deref())
                .execute(&pool).await.context("insert svi_update")?;
            }
        }
    }
    Ok(())
}
