//! Provides isolated stores and canonical message fixtures for tests.
#![allow(dead_code)]

use cursor_server::{
    model::{CanonicalMessage, Origin, Role},
    plugin::{PluginRegistry, PluginRuntime},
    remote::RemoteControl,
    store::Store,
};

pub async fn temp_store() -> (tempfile::TempDir, Store) {
    let directory = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", directory.path().join("test.db").display());
    let store = Store::connect(&url).await.unwrap();
    (directory, store)
}

pub fn remote(store: &Store) -> RemoteControl {
    let runtime = PluginRuntime::managed().unwrap();
    let plugins = PluginRegistry::managed(store.clone(), runtime, "0.1.0".into()).unwrap();
    RemoteControl::new(store.clone(), plugins, Default::default())
}

pub fn user(id: &str, text: &str) -> CanonicalMessage {
    CanonicalMessage::text(id, Role::User, Origin::User, text)
}
