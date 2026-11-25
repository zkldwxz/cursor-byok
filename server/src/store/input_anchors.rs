//! Deduplicates Cursor inputs against their Conversation base.
use crate::{
    model::{CheckpointId, ConversationId},
    Result,
};

use super::{now_ms, Store};

impl Store {
    /// Resolves the base checkpoint for one Cursor input id.
    ///
    /// The anchor exists so a retried input resolves to the lineage it was
    /// first attempted on: the first attempt may have committed the user
    /// message before failing, and re-anchoring to the advanced checkpoint
    /// would append that message a second time. It must never rewind the
    /// conversation, though — Cursor reuses a message id when the user resends
    /// with new context, and that state is not descended from the old anchor.
    pub async fn anchor_input(
        &self,
        conversation_id: &ConversationId,
        input_id: &str,
        base_checkpoint_id: CheckpointId,
    ) -> Result<CheckpointId> {
        let mut session = self.write_session().await?;
        let mut tx = session.begin().await?;
        sqlx::query(
            "INSERT INTO input_anchors
             (conversation_id, input_id, base_checkpoint_id, created_at_ms)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(conversation_id, input_id) DO NOTHING",
        )
        .bind(conversation_id.as_str())
        .bind(input_id)
        .bind(base_checkpoint_id.0)
        .bind(now_ms())
        .execute(&mut *tx)
        .await?;
        let anchored = sqlx::query_scalar::<_, i64>(
            "SELECT base_checkpoint_id FROM input_anchors
             WHERE conversation_id = ? AND input_id = ?",
        )
        .bind(conversation_id.as_str())
        .bind(input_id)
        .fetch_one(&mut *tx)
        .await?;
        let anchored = CheckpointId(anchored);
        if anchored != base_checkpoint_id
            && !is_ancestor_tx(&mut tx, conversation_id, anchored, base_checkpoint_id).await?
        {
            sqlx::query(
                "UPDATE input_anchors SET base_checkpoint_id = ?, created_at_ms = ?
                 WHERE conversation_id = ? AND input_id = ?",
            )
            .bind(base_checkpoint_id.0)
            .bind(now_ms())
            .bind(conversation_id.as_str())
            .bind(input_id)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            session.committed();
            return Ok(base_checkpoint_id);
        }
        tx.commit().await?;
        session.committed();
        Ok(anchored)
    }
}

/// Whether `ancestor` is `descendant` or one of its parents.
async fn is_ancestor_tx(
    tx: &mut sqlx::SqliteConnection,
    conversation_id: &ConversationId,
    ancestor: CheckpointId,
    descendant: CheckpointId,
) -> Result<bool> {
    let found: bool = sqlx::query_scalar(
        "WITH RECURSIVE lineage(checkpoint_id) AS (
             SELECT checkpoint_id FROM conversation_checkpoints
             WHERE checkpoint_id = ? AND conversation_id = ?
             UNION
             SELECT c.parent_checkpoint_id
             FROM conversation_checkpoints c
             JOIN lineage l ON c.checkpoint_id = l.checkpoint_id
             WHERE c.parent_checkpoint_id IS NOT NULL
         )
         SELECT EXISTS(SELECT 1 FROM lineage WHERE checkpoint_id = ?)",
    )
    .bind(descendant.0)
    .bind(conversation_id.as_str())
    .bind(ancestor.0)
    .fetch_one(&mut *tx)
    .await?;
    Ok(found)
}

#[cfg(test)]
mod tests {
    use crate::model::{CanonicalMessage, Origin, Role};

    use super::*;

    async fn store() -> Store {
        let directory = tempfile::tempdir().unwrap();
        Store::connect(&format!(
            "sqlite://{}",
            directory.path().join("test.db").display()
        ))
        .await
        .unwrap()
    }

    fn user(id: &str, text: &str) -> CanonicalMessage {
        CanonicalMessage::text(id, Role::User, Origin::User, text)
    }

    /// Importing a checkpoint does not advance the conversation head; a run
    /// commit does. Tests that model a later turn must move the head too,
    /// otherwise every import looks like a fresh branch off the root.
    async fn set_current(store: &Store, conversation: &ConversationId, checkpoint: CheckpointId) {
        sqlx::query("UPDATE conversations SET current_checkpoint_id = ? WHERE conversation_id = ?")
            .bind(checkpoint.0)
            .bind(conversation.as_str())
            .execute(store.pool())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_retry_after_the_attempt_committed_keeps_the_original_anchor() {
        let store = store().await;
        let conversation = ConversationId::new("conversation");
        let root = store.ensure_conversation(&conversation).await.unwrap();
        let first = store
            .import_checkpoint(&conversation, &[user("cursor-root:0", "one")])
            .await
            .unwrap();
        set_current(&store, &conversation, first).await;
        assert_eq!(
            store
                .anchor_input(&conversation, "input-1", first)
                .await
                .unwrap(),
            first
        );
        // The first attempt committed the user message before failing; the
        // retry arrives on the advanced checkpoint and must still dedupe
        // against the lineage it started.
        let advanced = store
            .import_checkpoint(
                &conversation,
                &[user("cursor-root:0", "one"), user("cursor-root:1", "two")],
            )
            .await
            .unwrap();
        assert_ne!(advanced, first);
        assert_ne!(advanced, root);
        assert_eq!(
            store
                .anchor_input(&conversation, "input-1", advanced)
                .await
                .unwrap(),
            first
        );
    }

    #[tokio::test]
    async fn an_anchor_on_another_branch_rebinds_instead_of_rewinding() {
        let store = store().await;
        let conversation = ConversationId::new("conversation");
        let root = store.ensure_conversation(&conversation).await.unwrap();
        let first = store
            .import_checkpoint(&conversation, &[user("cursor-root:0", "one")])
            .await
            .unwrap();
        set_current(&store, &conversation, first).await;
        assert_eq!(
            store
                .anchor_input(&conversation, "input-1", first)
                .await
                .unwrap(),
            first
        );
        // Cursor reused the message id for a resend with new context, so the
        // new state is a sibling of the first attempt, not a descendant.
        let resent = store
            .import_checkpoint(
                &conversation,
                &[user("cursor-root:0-resent", "one, with context")],
            )
            .await
            .unwrap();
        set_current(&store, &conversation, resent).await;
        assert_ne!(resent, first);
        assert_ne!(resent, root);
        assert_eq!(
            store
                .anchor_input(&conversation, "input-1", resent)
                .await
                .unwrap(),
            resent
        );
        // The rebound anchor is what later retries of this input resolve to.
        assert_eq!(
            store
                .anchor_input(&conversation, "input-1", resent)
                .await
                .unwrap(),
            resent
        );
    }
}
