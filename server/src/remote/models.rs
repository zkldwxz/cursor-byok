//! Lists the models a phone can choose from.
use serde::Serialize;

use crate::{plugin::PluginRegistry, store::Store, Result};

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(super) struct Model {
    pub id: String,
    pub label: String,
    /// As its maker names it; the label may be anything.
    pub name: String,
}

#[derive(Clone)]
pub(super) struct Models {
    store: Store,
    plugins: Option<PluginRegistry>,
}

impl Models {
    pub fn new(store: Store, plugins: Option<PluginRegistry>) -> Self {
        Self { store, plugins }
    }

    pub async fn list(&self) -> Result<Vec<Model>> {
        let configured = self.store.configured_models().await?;
        let plugins = match &self.plugins {
            Some(plugins) => plugins.configured_models().await,
            None => Vec::new(),
        };
        let configured = configured.into_iter().map(|model| Model {
            id: model.model_hash,
            label: model.display_name,
            name: model.model_id,
        });
        let plugins = plugins.into_iter().map(|model| Model {
            name: model.id.clone(),
            id: model.id,
            label: model.display_name,
        });
        Ok(configured.chain(plugins).collect())
    }
}
