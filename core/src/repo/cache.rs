use crate::dag::{EntityId, EntitySnapshot, OperationId};

/// Hook trait invoked around repository read/write events.
pub trait RepositoryCacheHook: Send + Sync {
    fn on_entity_loaded(&self, _entity_id: &EntityId, _snapshot: &EntitySnapshot) {}

    fn on_pack_persisted(&self, _entity_id: &EntityId, _inserted: &[OperationId]) {}

    fn invalidate_entity(&self, _entity_id: &EntityId) {}
}

/// Default no-op cache hook used when no caching is configured.
#[derive(Debug, Default)]
pub struct NoopCache;

impl RepositoryCacheHook for NoopCache {}
