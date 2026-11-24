//! Keeps prices the user has not set filled in from published rates.
mod models_dev;

use std::{sync::Arc, time::Duration};

use parking_lot::RwLock;

use crate::{store::Store, Result};

use models_dev::PriceCatalog;

const ENDPOINT: &str = "https://models.dev/api.json";
const REFRESH: Duration = Duration::from_secs(24 * 60 * 60);
const RETRY: Duration = Duration::from_secs(60 * 60);
const TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub(crate) struct DefaultPrices {
    store: Store,
    endpoint: String,
    catalog: Arc<RwLock<Option<Arc<PriceCatalog>>>>,
}

impl DefaultPrices {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            endpoint: ENDPOINT.into(),
            catalog: Arc::default(),
        }
    }

    pub async fn follow(&self) {
        loop {
            let wait = match self.refresh().await {
                Ok(()) => REFRESH,
                Err(error) => {
                    tracing::warn!(%error, "default model price refresh failed");
                    RETRY
                }
            };
            tokio::time::sleep(wait).await;
        }
    }

    async fn refresh(&self) -> Result<()> {
        let response = crate::network::client(&self.store)
            .await?
            .get(&self.endpoint)
            .timeout(TIMEOUT)
            .send()
            .await?
            .error_for_status()?;
        let catalog = PriceCatalog::parse(&response.bytes().await?)?;
        *self.catalog.write() = Some(Arc::new(catalog));
        self.fill().await
    }

    /// Prices models first called since the last refresh; does nothing until a catalog is loaded.
    pub async fn fill(&self) -> Result<()> {
        let Some(catalog) = self.catalog.read().clone() else {
            return Ok(());
        };
        self.store
            .fill_default_prices(|model_id| catalog.price(model_id))
            .await
    }
}

#[cfg(test)]
mod tests {
    use axum::{http::StatusCode, response::IntoResponse, routing::get, Json, Router};
    use serde_json::json;

    use super::*;
    use crate::model::{NewLlmCall, ProviderType};

    const URL: &str = "https://relay.example/v1/messages";

    async fn record(store: &Store, model_id: &str) {
        store
            .start_llm_call(&NewLlmCall {
                call_id: model_id.into(),
                run_id: "run".into(),
                conversation_id: "conversation".into(),
                provider_call_index: 0,
                model_hash: "hash".into(),
                provider_type: ProviderType::Anthropic,
                provider_url: URL.into(),
                request_type: ProviderType::Anthropic,
                request_url: URL.into(),
                model_id: model_id.into(),
                display_name: model_id.into(),
                reasoning_effort: None,
                fast: false,
                message_count: 1,
                tool_count: 0,
                detailed: false,
            })
            .await
            .unwrap();
    }

    async fn priced(store: &Store) -> Vec<String> {
        let mut models = store
            .model_price_entries()
            .await
            .unwrap()
            .into_iter()
            .filter(|entry| entry.price.is_some())
            .map(|entry| entry.model_id)
            .collect::<Vec<_>>();
        models.sort();
        models
    }

    #[tokio::test]
    async fn published_prices_reach_past_and_later_models_and_survive_an_outage() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::connect(&format!(
            "sqlite://{}",
            directory.path().join("test.db").display()
        ))
        .await
        .unwrap();
        let online = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let served = online.clone();
        let app = Router::new().route(
            "/api.json",
            get(move || {
                let online = served.load(std::sync::atomic::Ordering::SeqCst);
                async move {
                    if !online {
                        return StatusCode::SERVICE_UNAVAILABLE.into_response();
                    }
                    let model = |id: &str| json!({"canonical_model_id": format!("lab/{id}"), "cost": {"input": 1, "output": 2}});
                    Json(json!({"lab": {"models": {"first": model("first"), "second": model("second")}}}))
                        .into_response()
                }
            }),
        );
        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut prices = DefaultPrices::new(store.clone());
        prices.endpoint = format!("http://{}/api.json", socket.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(socket, app).await.unwrap() });
        record(&store, "first").await;

        assert!(prices.refresh().await.is_err());
        prices.fill().await.unwrap();
        assert!(priced(&store).await.is_empty());

        online.store(true, std::sync::atomic::Ordering::SeqCst);
        prices.refresh().await.unwrap();
        assert_eq!(priced(&store).await, ["first"]);

        online.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(prices.refresh().await.is_err());
        record(&store, "second").await;
        prices.fill().await.unwrap();
        assert_eq!(priced(&store).await, ["first", "second"]);
    }
}
