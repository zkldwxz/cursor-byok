//! Finds the SSH conversations Cursor's remote control leaves out, and reads them from what this server kept.
use std::path::PathBuf;

use sqlx::{sqlite::SqliteConnectOptions, ConnectOptions, Row as _};

use crate::{
    model::{CheckpointId, ConversationId},
    store::Store,
    Result,
};

use super::{models::Model, transcript::Transcript};

const SCHEME: &str = "vscode-remote";

#[derive(Clone, Debug, PartialEq)]
pub(super) struct SshChat {
    pub id: String,
    pub title: String,
    pub host: String,
    pub path: String,
    pub updated_ms: i64,
    pub running: bool,
}

impl SshChat {
    /// Keeps a folder of one host apart from the same folder of another.
    pub fn workspace(&self) -> String {
        format!("ssh://{}{}", self.host, self.path)
    }
}

/// A conversation as far as it was kept, and where that was.
pub(super) struct Kept {
    pub checkpoint: CheckpointId,
    pub running: bool,
    pub transcript: Transcript,
    pub model: Option<Model>,
}

#[derive(Clone)]
pub(super) struct SshChats {
    store: Store,
    /// Cursor's own state, which names every conversation.
    cursor: Option<PathBuf>,
}

impl SshChats {
    pub fn new(store: Store, cursor: Option<PathBuf>) -> Self {
        Self { store, cursor }
    }

    /// Only those this server ran: it has the others' names but not their words.
    pub async fn list(&self) -> Result<Vec<SshChat>> {
        let Some(cursor) = self.cursor.as_ref().filter(|path| path.is_file()) else {
            return Ok(Vec::new());
        };
        let mut connection = SqliteConnectOptions::new()
            .filename(cursor)
            .read_only(true)
            .connect()
            .await?;
        let rows = sqlx::query(
            "SELECT composerId, lastUpdatedAt,
                    json_extract(value, '$.name') AS name,
                    json_extract(value, '$.workspaceIdentifier.uri.authority') AS authority,
                    json_extract(value, '$.workspaceIdentifier.uri.path') AS path
             FROM composerHeaders
             WHERE coalesce(isArchived, 0) = 0 AND coalesce(isSubagent, 0) = 0
               AND json_extract(value, '$.workspaceIdentifier.uri.scheme') = ?",
        )
        .bind(SCHEME)
        .fetch_all(&mut connection)
        .await?;
        let mut chats = Vec::new();
        for row in rows {
            let id: String = row.get("composerId");
            let kept = self.store.conversation(&ConversationId::new(&id)).await?;
            let (Some(kept), Some(authority), Some(path)) = (
                kept,
                row.get::<Option<String>, _>("authority"),
                row.get::<Option<String>, _>("path"),
            ) else {
                continue;
            };
            let host = authority.split_once('+').map_or("", |(_, host)| host);
            chats.push(SshChat {
                id,
                title: row.get::<Option<String>, _>("name").unwrap_or_default(),
                host: host.into(),
                path,
                updated_ms: row
                    .get::<Option<i64>, _>("lastUpdatedAt")
                    .unwrap_or_default(),
                running: kept.active_run_id.is_some(),
            });
        }
        Ok(chats)
    }

    /// `None` when nothing was added to what was read last.
    pub async fn read(&self, id: &str, last: Option<(CheckpointId, bool)>) -> Result<Option<Kept>> {
        let id = ConversationId::new(id);
        let Some(conversation) = self.store.conversation(&id).await? else {
            return Ok(None);
        };
        let checkpoint = conversation.current_checkpoint_id;
        let running = conversation.active_run_id.is_some();
        if last == Some((checkpoint, running)) {
            return Ok(None);
        }
        let messages = self.store.load_checkpoint_messages(checkpoint).await?;
        let model = self.store.last_model(&id).await?;
        Ok(Some(Kept {
            checkpoint,
            running,
            transcript: super::kept::transcript(&messages, running),
            model: model.map(|(name, label)| Model {
                id: String::new(),
                label,
                name,
            }),
        }))
    }
}

#[cfg(test)]
pub(super) mod testing {
    use super::*;

    /// Cursor's state with these conversations: id, title, scheme, authority and path.
    pub async fn cursor_state(chats: &[(&str, &str, &str, &str, &str)]) -> PathBuf {
        let path = tempfile::tempdir().unwrap().keep().join("state.vscdb");
        let mut connection = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .connect()
            .await
            .unwrap();
        sqlx::query("CREATE TABLE composerHeaders (composerId TEXT PRIMARY KEY, lastUpdatedAt INTEGER, isArchived INTEGER, isSubagent INTEGER, value TEXT)")
            .execute(&mut connection)
            .await
            .unwrap();
        for (id, title, scheme, authority, folder) in chats {
            let value = serde_json::json!({
                "name": title,
                "workspaceIdentifier": {"uri": {"scheme": scheme, "authority": authority, "path": folder}},
            });
            sqlx::query("INSERT INTO composerHeaders VALUES (?, 7, 0, 0, ?)")
                .bind(id)
                .bind(value.to_string())
                .execute(&mut connection)
                .await
                .unwrap();
        }
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_ssh_conversations_this_server_ran_are_listed() {
        let directory = tempfile::tempdir().unwrap().keep();
        let url = format!("sqlite://{}", directory.join("state.db").display());
        let store = Store::connect(&url).await.unwrap();
        for id in ["far", "near"] {
            store
                .ensure_conversation(&ConversationId::new(id))
                .await
                .unwrap();
        }
        let cursor = testing::cursor_state(&[
            ("far", "deploy", SCHEME, "ssh-remote+box", "/srv/app"),
            ("unknown", "other", SCHEME, "ssh-remote+box", "/srv/app"),
            ("near", "local", "file", "", "/projects/shop"),
        ])
        .await;
        let chats = SshChats::new(store.clone(), Some(cursor)).list().await;
        let chats = chats.unwrap();
        assert_eq!(chats.len(), 1);
        assert_eq!(chats[0].title, "deploy");
        assert_eq!(chats[0].workspace(), "ssh://box/srv/app");
        assert!(!chats[0].running);

        let elsewhere = Some(PathBuf::from("/nowhere/state.vscdb"));
        let none = SshChats::new(store, elsewhere).list().await.unwrap();
        assert!(none.is_empty());
    }
}
