use super::{MatchCandidates, MatchableChange, MatcherError, MatcherStmt};
use crate::{api::QueryEvent, updates::HandleMetrics};
use std::collections::HashMap;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::{CancellationToken, WaitForCancellationFuture};
use tracing::{error, info, trace};
use uuid::Uuid;

use corro_api_types::{ChangeId, ColumnName, RowId, SqliteValue};
use rusqlite::Connection;
use sqlite_pool::RusqlitePool;
use sqlite3_parser::ast::ResultColumn;

#[derive(Clone, Debug)]
pub struct MatcherHandle {
    pub(super) inner: std::sync::Arc<InnerMatcherHandle>,
    pub(super) state: super::StateLock,
}

#[derive(Debug)]
pub(super) struct InnerMatcherHandle {
    pub(super) id: Uuid,
    pub(super) sql: String,
    pub(super) hash: String,
    pub(super) pool: sqlite_pool::RusqlitePool,
    pub(super) parsed: super::db::ParsedSelect,
    pub(super) col_names: Vec<ColumnName>,
    pub(super) cancel: CancellationToken,
    pub(super) changes_tx: mpsc::Sender<MatchCandidates>,
    pub(super) last_change_rx: watch::Receiver<ChangeId>,
    pub(super) purge_tx: mpsc::Sender<oneshot::Sender<rusqlite::Result<usize>>>,
    // some state from the matcher so we can take a look later
    pub(super) subs_path: String,
    pub(super) cached_statements: HashMap<String, MatcherStmt>,
    pub(super) metrics: HashMap<String, HandleMetrics>,
}

#[async_trait::async_trait]
impl crate::updates::Handle for MatcherHandle {
    fn id(&self) -> Uuid {
        self.inner.id
    }

    fn cancelled(&self) -> WaitForCancellationFuture<'_> {
        self.inner.cancel.cancelled()
    }

    fn changes_tx(&self) -> mpsc::Sender<MatchCandidates> {
        self.inner.changes_tx.clone()
    }

    async fn cleanup(&self) {
        self.inner.cancel.cancel();
        info!(sub_id = %self.inner.id, "Canceled subscription");
    }

    fn filter_matchable_change(
        &self,
        candidates: &mut MatchCandidates,
        change: MatchableChange,
    ) -> bool {
        trace!("filtering change {change:?}");
        // don't double process the same pk
        if candidates
            .get(change.table)
            .map(|pks| pks.contains_key(change.pk))
            .unwrap_or_default()
        {
            trace!("already contained key");
            return false;
        }

        // don't consider changes that don't have both the table + col in the matcher query
        if !self
            .inner
            .parsed
            .table_columns
            .get(change.table.as_str())
            .map(|cols| change.column.is_crsql_sentinel() || cols.contains(change.column.as_str()))
            .unwrap_or_default()
        {
            trace!("could not match against parsed query table and columns");
            return false;
        }

        if let Some(v) = candidates.get_mut(change.table) {
            v.insert(change.pk.to_vec(), change.cl).is_none()
        } else {
            candidates.insert(
                change.table.clone(),
                [(change.pk.to_vec(), change.cl)].into(),
            );
            true
        }
    }

    fn get_counter(&self, table: &str) -> &HandleMetrics {
        self.inner.metrics.get(table).unwrap_or_else(|| {
            panic!(
                "metrics counter for table '{}' missing. subs hash {}!",
                self.inner.hash, table
            )
        })
    }
}

impl MatcherHandle {
    pub fn sql(&self) -> &String {
        &self.inner.sql
    }

    pub fn hash(&self) -> &str {
        &self.inner.hash
    }

    pub fn parsed_columns(&self) -> &[ResultColumn] {
        &self.inner.parsed.columns
    }

    pub fn col_names(&self) -> &[ColumnName] {
        &self.inner.col_names
    }

    pub fn subs_path(&self) -> &String {
        &self.inner.subs_path
    }

    pub fn cached_stmts(&self) -> &HashMap<String, MatcherStmt> {
        &self.inner.cached_statements
    }

    pub fn pool(&self) -> &RusqlitePool {
        &self.inner.pool
    }

    fn wait_for_running_state(&self) {
        let (lock, cvar) = &*self.state;
        let mut state = lock.lock();
        while !state.is_running() {
            cvar.wait(&mut state);
        }
    }

    pub fn max_change_id(&self, conn: &Connection) -> rusqlite::Result<ChangeId> {
        self.wait_for_running_state();
        let mut prepped = conn.prepare_cached("SELECT COALESCE(MAX(id), 0) FROM changes")?;
        prepped.query_row([], |row| row.get(0))
    }

    pub fn last_change_id_sent(&self) -> ChangeId {
        *self.inner.last_change_rx.borrow()
    }

    pub fn max_row_id(&self, conn: &Connection) -> rusqlite::Result<RowId> {
        self.wait_for_running_state();
        let mut prepped =
            conn.prepare_cached("SELECT COALESCE(MAX(__corro_rowid), 0) FROM query")?;
        prepped.query_row([], |row| row.get(0))
    }

    /// Purges old changes from the subscription's changes table, keeping only the most recent 500.
    /// Returns the number of rows deleted.
    ///
    /// This method sends a request to the Matcher task to perform the purge.
    pub async fn purge_old_changes(&self) -> rusqlite::Result<usize> {
        self.wait_for_running_state();

        let (tx, rx) = oneshot::channel();

        // Send purge request to the Matcher task
        self.inner
            .purge_tx
            .send(tx)
            .await
            .map_err(|_| rusqlite::Error::InvalidQuery)?;

        // Wait for the result
        rx.await.map_err(|_| rusqlite::Error::InvalidQuery)?
    }

    pub fn changes_since(
        &self,
        since: ChangeId,
        conn: &Connection,
        tx: mpsc::Sender<QueryEvent>,
    ) -> rusqlite::Result<ChangeId> {
        self.wait_for_running_state();

        let mut prepped = conn.prepare_cached("SELECT COALESCE(MIN(id), 0) FROM changes")?;
        let min_change_id: u64 = prepped.query_row([], |row| row.get(0))?;

        // return error if we've cleared changes after the received change id
        if since.0 + 1 < min_change_id {
            return Err(rusqlite::Error::ModuleError(format!(
                "subscription already deleted older changes, min change id: {min_change_id}",
            )));
        }

        let mut query_cols = vec![];
        for i in 0..(self.parsed_columns().len()) {
            query_cols.push(format!("col_{i}"));
        }
        let mut prepped = conn.prepare_cached(&format!(
            "SELECT id, type, __corro_rowid, {} FROM changes WHERE id > ? ORDER BY id ASC",
            query_cols.join(",")
        ))?;

        let col_count = prepped.column_count();

        let mut max_change_id = since;

        let mut rows = prepped.query([since])?;

        loop {
            let row = match rows.next()? {
                Some(row) => row,
                None => break,
            };

            let change_id: ChangeId = row.get(0)?;
            if change_id.0 > max_change_id.0 {
                max_change_id = change_id;
            }

            if let Err(e) = tx.blocking_send(QueryEvent::Change(
                row.get(1)?,
                row.get(2)?,
                (3..col_count)
                    .map(|i| row.get::<_, SqliteValue>(i))
                    .collect::<rusqlite::Result<Vec<_>>>()?,
                change_id,
            )) {
                error!("could not send change to channel: {e}");
                break;
            }
        }

        Ok(max_change_id)
    }

    pub fn all_rows(
        &self,
        conn: &Connection,
        tx: mpsc::Sender<QueryEvent>,
    ) -> Result<ChangeId, MatcherError> {
        self.wait_for_running_state();
        let mut query_cols = vec![];
        for i in 0..(self.parsed_columns().len()) {
            query_cols.push(format!("col_{i}"));
        }
        let mut prepped = conn.prepare_cached(&format!(
            "SELECT __corro_rowid, {} FROM query",
            query_cols.join(",")
        ))?;

        let col_count = prepped.column_count();

        tx.blocking_send(QueryEvent::Columns(self.col_names().to_vec()))
            .map_err(|_| MatcherError::EventReceiverClosed)?;

        let start = std::time::Instant::now();
        let mut rows = prepped.query([])?;
        let elapsed = start.elapsed();

        let mut count = 0;

        loop {
            let row = match rows.next()? {
                Some(row) => row,
                None => break,
            };

            tx.blocking_send(QueryEvent::Row(
                row.get(0)?,
                (1..col_count)
                    .map(|i| row.get::<_, SqliteValue>(i))
                    .collect::<rusqlite::Result<Vec<_>>>()?,
            ))
            .map_err(|_| MatcherError::EventReceiverClosed)?;
            count += 1;
        }

        trace!("sent {count} rows");

        let max_change_id = conn
            .prepare("SELECT COALESCE(MAX(id),0) FROM changes")?
            .query_row([], |row| row.get(0))?;

        tx.blocking_send(QueryEvent::EndOfQuery {
            time: elapsed.as_secs_f64(),
            change_id: Some(max_change_id),
        })
        .map_err(|_| MatcherError::EventReceiverClosed)?;

        Ok(max_change_id)
    }
}
