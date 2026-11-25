//! Persists immutable internal Conversation checkpoints.
use sha2::{Digest, Sha256};
use sqlx::SqliteConnection;

use crate::{
    model::{CanonicalMessage, CheckpointId, ConversationId, RunId},
    Error, Result,
};

use super::{now_ms, Store};

impl Store {
    pub async fn ensure_conversation(
        &self,
        conversation_id: &ConversationId,
    ) -> Result<CheckpointId> {
        let mut session = self.write_session().await?;
        let mut tx = session.begin().await?;
        let checkpoint = Self::ensure_conversation_tx(&mut tx, conversation_id).await?;
        tx.commit().await?;
        session.committed();
        Ok(checkpoint)
    }

    pub async fn load_checkpoint_messages(
        &self,
        checkpoint_id: CheckpointId,
    ) -> Result<Vec<CanonicalMessage>> {
        let mut connection = self.pool.acquire().await?;
        Self::load_checkpoint_messages_tx(&mut connection, checkpoint_id.0).await
    }

    pub async fn checkpoint_parent(
        &self,
        checkpoint_id: CheckpointId,
    ) -> Result<Option<CheckpointId>> {
        let parent = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT parent_checkpoint_id FROM conversation_checkpoints WHERE checkpoint_id = ?",
        )
        .bind(checkpoint_id.0)
        .fetch_optional(&self.pool)
        .await?
        .flatten()
        .map(CheckpointId);
        Ok(parent)
    }

    pub async fn load_current_messages(
        &self,
        conversation_id: &ConversationId,
    ) -> Result<Vec<CanonicalMessage>> {
        let Some(checkpoint_id) = sqlx::query_scalar::<_, i64>(
            "SELECT current_checkpoint_id FROM conversations WHERE conversation_id = ?",
        )
        .bind(conversation_id.as_str())
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(Vec::new());
        };
        self.load_checkpoint_messages(CheckpointId(checkpoint_id))
            .await
    }

    pub async fn match_checkpoint_prefix(
        &self,
        conversation_id: &ConversationId,
        base_checkpoint_id: CheckpointId,
        additions: &[CanonicalMessage],
    ) -> Result<(CheckpointId, usize)> {
        let mut checkpoint = base_checkpoint_id;
        let mut messages = self.load_checkpoint_messages(checkpoint).await?;
        for (index, addition) in additions.iter().enumerate() {
            messages.push(addition.clone());
            let digest = message_digest(&messages)?;
            let child = sqlx::query_scalar::<_, i64>(
                "SELECT checkpoint_id FROM conversation_checkpoints
                 WHERE conversation_id = ? AND parent_checkpoint_id = ? AND state_digest = ?",
            )
            .bind(conversation_id.as_str())
            .bind(checkpoint.0)
            .bind(digest.as_slice())
            .fetch_optional(&self.pool)
            .await?
            .map(CheckpointId);
            let Some(child) = child else {
                return Ok((checkpoint, index));
            };
            if self.load_checkpoint_messages(child).await? != messages {
                return Ok((checkpoint, index));
            }
            checkpoint = child;
        }
        Ok((checkpoint, additions.len()))
    }

    pub async fn import_checkpoint(
        &self,
        conversation_id: &ConversationId,
        messages: &[CanonicalMessage],
    ) -> Result<CheckpointId> {
        let digest = message_digest(messages)?;
        let mut session = self.write_session().await?;
        let mut tx = session.begin().await?;
        let current = Self::ensure_conversation_tx(&mut tx, conversation_id).await?;
        if let Some(existing) = sqlx::query_scalar::<_, i64>(
            "SELECT checkpoint_id FROM conversation_checkpoints
             WHERE conversation_id = ? AND state_digest = ?",
        )
        .bind(conversation_id.as_str())
        .bind(digest.as_slice())
        .fetch_optional(&mut *tx)
        .await?
        {
            tx.commit().await?;
            session.committed();
            return Ok(CheckpointId(existing));
        }

        let current_messages = Self::load_checkpoint_messages_tx(&mut tx, current.0).await?;
        // Cursor re-encodes the history it sends back with its own root ids and
        // its own tool-round grouping, so comparing message ids would make every
        // turn look like a new branch off the root and re-insert the whole
        // history. Compare what the provider actually sees instead.
        let common = provider_prefix_len(&current_messages, messages)?;
        if common == current_messages.len() && common == messages.len() {
            tx.commit().await?;
            session.committed();
            return Ok(current);
        }
        let (parent, additions) = if common == current_messages.len() {
            (current, &messages[common..])
        } else {
            let root: i64 = sqlx::query_scalar(
                "SELECT checkpoint_id FROM conversation_checkpoints
                 WHERE conversation_id = ? AND parent_checkpoint_id IS NULL",
            )
            .bind(conversation_id.as_str())
            .fetch_one(&mut *tx)
            .await?;
            (CheckpointId(root), messages)
        };
        let checkpoint =
            Self::insert_checkpoint_tx(&mut tx, conversation_id, parent, additions, digest).await?;
        tx.commit().await?;
        session.committed();
        Ok(checkpoint)
    }

    pub async fn append_checkpoint(
        &self,
        conversation_id: &ConversationId,
        run_id: &RunId,
        expected: CheckpointId,
        additions: &[CanonicalMessage],
    ) -> Result<CheckpointId> {
        if additions.is_empty() {
            return Ok(expected);
        }
        let mut full = self.load_checkpoint_messages(expected).await?;
        full.extend_from_slice(additions);
        let digest = message_digest(&full)?;
        let mut session = self.write_session().await?;
        let mut tx = session.begin().await?;
        let checkpoint = Self::append_checkpoint_with_digest_tx(
            &mut tx,
            conversation_id,
            run_id,
            expected,
            additions,
            digest,
        )
        .await?;
        tx.commit().await?;
        session.committed();
        Ok(checkpoint)
    }

    pub async fn replace_checkpoint(
        &self,
        conversation_id: &ConversationId,
        run_id: &RunId,
        expected: CheckpointId,
        messages: &[CanonicalMessage],
    ) -> Result<CheckpointId> {
        let digest = message_digest(messages)?;
        let mut session = self.write_session().await?;
        let mut tx = session.begin().await?;
        Self::require_active_head_tx(&mut tx, conversation_id, run_id, expected).await?;
        let root: i64 = sqlx::query_scalar(
            "SELECT checkpoint_id FROM conversation_checkpoints
             WHERE conversation_id = ? AND parent_checkpoint_id IS NULL",
        )
        .bind(conversation_id.as_str())
        .fetch_one(&mut *tx)
        .await?;
        let checkpoint = Self::insert_checkpoint_tx(
            &mut tx,
            conversation_id,
            CheckpointId(root),
            messages,
            digest,
        )
        .await?;
        let updated = sqlx::query(
            "UPDATE conversations SET current_checkpoint_id = ?, updated_at_ms = ?
             WHERE conversation_id = ? AND current_checkpoint_id = ? AND active_run_id = ?",
        )
        .bind(checkpoint.0)
        .bind(now_ms())
        .bind(conversation_id.as_str())
        .bind(expected.0)
        .bind(run_id.as_str())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if updated != 1 {
            return Err(Error::Store(format!(
                "lost active ownership while replacing checkpoint for run {run_id}"
            )));
        }
        sqlx::query("UPDATE runs SET head_checkpoint_id = ?, updated_at_ms = ? WHERE run_id = ?")
            .bind(checkpoint.0)
            .bind(now_ms())
            .bind(run_id.as_str())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        session.committed();
        Ok(checkpoint)
    }

    pub async fn append_message_once(
        &self,
        conversation_id: &ConversationId,
        run_id: &RunId,
        expected: CheckpointId,
        message: &CanonicalMessage,
    ) -> Result<(CheckpointId, bool)> {
        let existing = self.load_checkpoint_messages(expected).await?;
        if let Some(existing) = existing.iter().find(|existing| {
            existing.message_id == message.message_id
                || message.runtime_event_id.is_some()
                    && existing.runtime_event_id == message.runtime_event_id
        }) {
            return if existing == message {
                Ok((expected, false))
            } else {
                Err(Error::Store(format!(
                    "message id or runtime event reused with different content: {}",
                    message.message_id
                )))
            };
        }
        Ok((
            self.append_checkpoint(
                conversation_id,
                run_id,
                expected,
                std::slice::from_ref(message),
            )
            .await?,
            true,
        ))
    }

    pub(crate) async fn append_checkpoint_tx(
        tx: &mut SqliteConnection,
        conversation_id: &ConversationId,
        run_id: &RunId,
        expected: CheckpointId,
        additions: &[CanonicalMessage],
    ) -> Result<CheckpointId> {
        let mut full = Self::load_checkpoint_messages_tx(tx, expected.0).await?;
        full.extend_from_slice(additions);
        let digest = message_digest(&full)?;
        Self::append_checkpoint_with_digest_tx(
            tx,
            conversation_id,
            run_id,
            expected,
            additions,
            digest,
        )
        .await
    }

    async fn append_checkpoint_with_digest_tx(
        tx: &mut SqliteConnection,
        conversation_id: &ConversationId,
        run_id: &RunId,
        expected: CheckpointId,
        additions: &[CanonicalMessage],
        digest: [u8; 32],
    ) -> Result<CheckpointId> {
        Self::require_active_head_tx(tx, conversation_id, run_id, expected).await?;
        if sqlx::query_scalar::<_, i64>(
            "SELECT checkpoint_id FROM conversation_checkpoints
             WHERE conversation_id = ? AND state_digest = ?",
        )
        .bind(conversation_id.as_str())
        .bind(digest.as_slice())
        .fetch_optional(&mut *tx)
        .await?
        .is_some()
        {
            return Err(Error::Store(
                "active append would reuse an existing checkpoint instead of creating a child"
                    .into(),
            ));
        }
        let checkpoint =
            Self::insert_checkpoint_tx(tx, conversation_id, expected, additions, digest).await?;
        let updated = sqlx::query(
            "UPDATE conversations SET current_checkpoint_id = ?, updated_at_ms = ?
             WHERE conversation_id = ? AND current_checkpoint_id = ? AND active_run_id = ?",
        )
        .bind(checkpoint.0)
        .bind(now_ms())
        .bind(conversation_id.as_str())
        .bind(expected.0)
        .bind(run_id.as_str())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if updated != 1 {
            return Err(Error::Store(format!(
                "lost active ownership while appending checkpoint for run {run_id}"
            )));
        }
        sqlx::query("UPDATE runs SET head_checkpoint_id = ?, updated_at_ms = ? WHERE run_id = ?")
            .bind(checkpoint.0)
            .bind(now_ms())
            .bind(run_id.as_str())
            .execute(&mut *tx)
            .await?;
        Ok(checkpoint)
    }

    async fn insert_checkpoint_tx(
        tx: &mut SqliteConnection,
        conversation_id: &ConversationId,
        parent: CheckpointId,
        additions: &[CanonicalMessage],
        digest: [u8; 32],
    ) -> Result<CheckpointId> {
        for message in additions {
            Self::put_message_tx(tx, conversation_id, message).await?;
        }
        let checkpoint = sqlx::query(
            "INSERT INTO conversation_checkpoints
             (conversation_id, parent_checkpoint_id, state_digest, created_at_ms)
             VALUES (?, ?, ?, ?)",
        )
        .bind(conversation_id.as_str())
        .bind(parent.0)
        .bind(digest.as_slice())
        .bind(now_ms())
        .execute(&mut *tx)
        .await?
        .last_insert_rowid();
        for (ordinal, message) in additions.iter().enumerate() {
            sqlx::query(
                "INSERT INTO checkpoint_messages(checkpoint_id, ordinal, conversation_id, message_id)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(checkpoint)
            .bind(ordinal as i64)
            .bind(conversation_id.as_str())
            .bind(&message.message_id)
            .execute(&mut *tx)
            .await?;
        }
        Ok(CheckpointId(checkpoint))
    }
}

pub(crate) fn message_digest(messages: &[CanonicalMessage]) -> Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    for message in messages {
        let bytes = serde_json::to_vec(message)?;
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
    }
    Ok(hasher.finalize().into())
}

/// Length of the canonical prefix of `stored` whose provider projection equals
/// the projection of the same prefix of `incoming`.
///
/// Projection groups assistant tool calls with their results, so a message
/// cannot be compared in isolation: the cut has to land on a projection
/// boundary. Returns the canonical index just past the last matching group.
fn provider_prefix_len(
    stored: &[CanonicalMessage],
    incoming: &[CanonicalMessage],
) -> Result<usize> {
    let stored_projection = crate::model::project_messages_with_ends(stored)?;
    let incoming_projection = crate::model::project_messages_with_ends(incoming)?;
    let mut cut = 0;
    for ((left, stored_end), (right, _)) in stored_projection.iter().zip(incoming_projection.iter())
    {
        // `ProjectedMessage` carries the internal message id, which Cursor
        // re-encodes on every turn; only the provider-visible body decides
        // whether the histories agree.
        if left.role != right.role || left.content != right.content {
            break;
        }
        cut = *stored_end;
    }
    Ok(cut)
}

#[cfg(test)]
mod tests {
    use crate::model::{
        CanonicalMessage, MessageContent, Origin, Role, ToolCallContent, ToolResultContent,
        ToolRoundId,
    };

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

    fn assistant(id: &str, round: Option<&str>, call_id: Option<&str>) -> CanonicalMessage {
        CanonicalMessage {
            message_id: id.into(),
            role: Role::Assistant,
            origin: Origin::Assistant,
            content: MessageContent::Assistant {
                text: "answer".into(),
                thinking: String::new(),
                tool_round_id: round.map(ToolRoundId::new),
                replay_state: None,
                tool_calls: call_id
                    .map(|call_id| {
                        vec![ToolCallContent {
                            index: 0,
                            call_id: call_id.into(),
                            name: "Read".into(),
                            arguments: serde_json::json!({"path": "/a"}),
                        }]
                    })
                    .unwrap_or_default(),
            },
            runtime_event_id: None,
        }
    }

    fn tool_result(id: &str, call_id: &str) -> CanonicalMessage {
        CanonicalMessage {
            message_id: id.into(),
            role: Role::Tool,
            origin: Origin::Tool,
            content: MessageContent::ToolResult(ToolResultContent {
                call_id: call_id.into(),
                name: "Read".into(),
                content: "ok".into(),
                is_error: false,
                image: None,
                provider_parts: Vec::new(),
            }),
            runtime_event_id: None,
        }
    }

    async fn set_current(store: &Store, conversation: &ConversationId, checkpoint: CheckpointId) {
        sqlx::query("UPDATE conversations SET current_checkpoint_id = ? WHERE conversation_id = ?")
            .bind(checkpoint.0)
            .bind(conversation.as_str())
            .execute(store.pool())
            .await
            .unwrap();
    }

    async fn checkpoint_rows(store: &Store, conversation: &ConversationId) -> i64 {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM conversation_checkpoints WHERE conversation_id = ?",
        )
        .bind(conversation.as_str())
        .fetch_one(store.pool())
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_reencoded_history_appends_only_the_new_messages() {
        let store = store().await;
        let conversation = ConversationId::new("conversation");
        store.ensure_conversation(&conversation).await.unwrap();
        let first = store
            .import_checkpoint(
                &conversation,
                &[user("cursor-root:0", "one"), assistant("a1", None, None)],
            )
            .await
            .unwrap();
        set_current(&store, &conversation, first).await;
        let before = checkpoint_rows(&store, &conversation).await;

        // Cursor sends the same conversation back with its own ids.
        let second = store
            .import_checkpoint(
                &conversation,
                &[
                    user("cursor-root:0", "one"),
                    assistant("cursor-root:1", None, None),
                    user("cursor-root:2", "two"),
                ],
            )
            .await
            .unwrap();
        assert_ne!(second, first);
        let rows: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT checkpoint_id, parent_checkpoint_id FROM conversation_checkpoints
             WHERE conversation_id = ? ORDER BY checkpoint_id",
        )
        .bind(conversation.as_str())
        .fetch_all(store.pool())
        .await
        .unwrap();
        assert_eq!(rows.len() as i64, before + 1, "one new checkpoint only");
        let (_, parent) = *rows.last().unwrap();
        assert_eq!(
            parent, first.0,
            "the new checkpoint extends the current head"
        );
        let added: Vec<String> = sqlx::query_scalar(
            "SELECT message_id FROM checkpoint_messages WHERE checkpoint_id = ?
             ORDER BY ordinal",
        )
        .bind(second.0)
        .fetch_all(store.pool())
        .await
        .unwrap();
        assert_eq!(added, ["cursor-root:2"]);
    }

    #[tokio::test]
    async fn a_tool_round_is_compared_as_one_provider_group() {
        let store = store().await;
        let conversation = ConversationId::new("conversation");
        store.ensure_conversation(&conversation).await.unwrap();
        let first = store
            .import_checkpoint(
                &conversation,
                &[
                    user("cursor-root:0", "one"),
                    assistant("assistant-1", Some("round-1"), Some("call-1")),
                    tool_result("result-1", "call-1"),
                ],
            )
            .await
            .unwrap();
        set_current(&store, &conversation, first).await;

        // The same round arrives regrouped under Cursor's own ids.
        let second = store
            .import_checkpoint(
                &conversation,
                &[
                    user("cursor-root:0", "one"),
                    assistant("cursor-root:1", Some("cursor-round"), Some("call-1")),
                    tool_result("cursor-root:2", "call-1"),
                    user("cursor-root:3", "two"),
                ],
            )
            .await
            .unwrap();
        let added: Vec<String> = sqlx::query_scalar(
            "SELECT message_id FROM checkpoint_messages WHERE checkpoint_id = ?
             ORDER BY ordinal",
        )
        .bind(second.0)
        .fetch_all(store.pool())
        .await
        .unwrap();
        assert_eq!(added, ["cursor-root:3"]);
    }
}
