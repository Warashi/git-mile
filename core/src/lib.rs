pub mod clock;
pub mod dag;
pub mod error;
pub mod identity;
pub mod mile;
pub mod repo;

pub use clock::{LamportClock, LamportTimestamp, ReplicaId};
pub use dag::{
    BlobRef, EntityId, EntitySnapshot, EntityStore, EntitySummary, MergeOutcome, MergeStrategy,
    Operation, OperationBlob, OperationId, OperationMetadata, OperationPack, PackPersistResult,
};
pub use error::{Error, Result};
pub use identity::{
    AddProtectionInput, AddProtectionOutcome, AdoptIdentityInput, AdoptIdentityOutcome,
    CreateIdentityInput, IdentityEvent, IdentityEventKind, IdentityId, IdentityProtection,
    IdentitySnapshot, IdentityStatus, IdentityStore, IdentitySummary, ProtectionKind,
};
pub use mile::{
    AppendCommentInput, AppendCommentOutcome, ChangeStatusInput, ChangeStatusOutcome, CommentId,
    CreateMileInput, LabelId, MileComment, MileCommentAppended, MileEvent, MileEventKind, MileId,
    MileLabelAttached, MileLabelDetached, MileSnapshot, MileStatus, MileStatusChanged, MileStore,
    MileSummary, UpdateLabelsInput, UpdateLabelsOutcome,
};
pub use repo::{LockMode, NoopCache, RepositoryCacheHook, RepositoryLock, RepositoryLockGuard};

pub const APP_NAME: &str = "git-mile";

pub fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_application_identity() {
        assert_eq!(APP_NAME, "git-mile");
        assert_eq!(app_version(), env!("CARGO_PKG_VERSION"));
    }
}
