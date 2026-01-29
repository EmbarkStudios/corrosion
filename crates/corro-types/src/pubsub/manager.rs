use super::{Matcher, MatcherError, MatcherHandle, MatcherLoopConfig};
use crate::{agent::SplitPool, api::QueryEvent, schema::Schema, updates::Handle};
use camino::Utf8Path;
use std::collections::{BTreeMap, HashMap};
use tokio::sync::mpsc;
use tracing::error;
use tripwire::Tripwire;
use uuid::Uuid;

#[derive(Debug, Default, Clone)]
pub struct SubsManager(std::sync::Arc<parking_lot::RwLock<InnerSubsManager>>);

#[derive(Debug, Default)]
struct InnerSubsManager {
    handles: BTreeMap<Uuid, MatcherHandle>,
    queries: HashMap<String, Uuid>,
}

impl InnerSubsManager {
    fn get(&self, id: &Uuid) -> Option<MatcherHandle> {
        self.handles.get(id).cloned()
    }

    fn get_by_query(&self, sql: &str) -> Option<MatcherHandle> {
        self.queries
            .get(sql)
            .and_then(|id| self.handles.get(id).cloned())
    }

    pub fn get_by_hash(&self, hash: &str) -> Option<MatcherHandle> {
        self.handles
            .values()
            .find(|x| x.inner.hash == hash)
            .cloned()
    }

    fn remove(&mut self, id: &Uuid) -> Option<MatcherHandle> {
        let handle = self.handles.remove(id)?;
        self.queries.remove(&handle.inner.sql);
        Some(handle)
    }
}

// tools to bootstrap a new subscriber or notifier
pub struct MatcherCreated {
    pub evt_rx: mpsc::Receiver<QueryEvent>,
}

const SUB_EVENT_CHANNEL_CAP: usize = 512;

impl crate::updates::Manager<MatcherHandle> for SubsManager {
    fn trait_type(&self) -> String {
        "subs".to_string()
    }

    fn get(&self, id: &Uuid) -> Option<MatcherHandle> {
        self.0.read().get(id)
    }

    fn remove(&self, id: &Uuid) -> Option<MatcherHandle> {
        let mut inner = self.0.write();
        inner.remove(id)
    }

    fn get_handles(&self) -> BTreeMap<Uuid, MatcherHandle> {
        self.0.read().handles.clone()
    }
}

impl SubsManager {
    pub fn get(&self, id: &Uuid) -> Option<MatcherHandle> {
        self.0.read().get(id)
    }

    pub fn get_by_query(&self, sql: &str) -> Option<MatcherHandle> {
        self.0.read().get_by_query(sql)
    }

    pub fn get_by_hash(&self, hash: &str) -> Option<MatcherHandle> {
        self.0.read().get_by_hash(hash)
    }

    pub fn get_handles(&self) -> BTreeMap<Uuid, MatcherHandle> {
        self.0.read().handles.clone()
    }

    pub async fn drop_handles(&self) {
        let handles = {
            let mut inner = self.0.write();
            std::mem::take(&mut inner.handles)
        };
        for (_, handle) in handles.iter() {
            handle.cleanup().await;
        }
    }

    pub fn get_or_insert(
        &self,
        sql: &str,
        subs_path: &Utf8Path,
        schema: &Schema,
        pool: &SplitPool,
        tripwire: Tripwire,
        loop_cfg: MatcherLoopConfig,
    ) -> Result<(MatcherHandle, Option<MatcherCreated>), MatcherError> {
        if let Some(handle) = self.get_by_query(sql) {
            return Ok((handle, None));
        }

        let mut inner = self.0.write();
        if let Some(handle) = inner.get_by_query(sql) {
            return Ok((handle, None));
        }

        let id = Uuid::new_v4();
        let (evt_tx, evt_rx) = mpsc::channel(SUB_EVENT_CHANNEL_CAP);

        let handle_res = Matcher::create(
            id,
            subs_path.to_path_buf(),
            schema,
            pool.client_dedicated()?,
            evt_tx,
            sql,
            tripwire,
            loop_cfg,
        );

        let handle = match handle_res {
            Ok(handle) => handle,
            Err(e) => {
                error!(sub_id = %id, "could not create subscription: {e}");
                if let Err(e) = Matcher::cleanup(id, &Matcher::sub_path(subs_path, id)) {
                    error!("could not cleanup subscription: {e}");
                }

                return Err(e);
            }
        };

        inner.handles.insert(id, handle.clone());
        inner.queries.insert(sql.to_owned(), id);

        Ok((handle, Some(MatcherCreated { evt_rx })))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn restore(
        &self,
        id: Uuid,
        subs_path: &Utf8Path,
        schema: &Schema,
        pool: &SplitPool,
        tripwire: Tripwire,
        loop_cfg: MatcherLoopConfig,
    ) -> Result<(MatcherHandle, MatcherCreated), MatcherError> {
        let mut inner = self.0.write();

        if inner.handles.contains_key(&id) {
            return Err(MatcherError::CannotRestoreExisting);
        }

        let (evt_tx, evt_rx) = mpsc::channel(SUB_EVENT_CHANNEL_CAP);

        let handle = Matcher::restore(
            id,
            subs_path.to_path_buf(),
            schema,
            pool.client_dedicated()?,
            evt_tx,
            tripwire,
            loop_cfg,
        )?;

        inner.handles.insert(id, handle.clone());
        inner.queries.insert(handle.inner.sql.clone(), id);

        Ok((handle, MatcherCreated { evt_rx }))
    }

    pub fn remove(&self, id: &Uuid) -> Option<MatcherHandle> {
        let mut inner = self.0.write();
        inner.remove(id)
    }
}
