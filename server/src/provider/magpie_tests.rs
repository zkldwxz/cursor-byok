//! Exercises gateway discovery and catalog changes against an isolated local gateway.
use super::*;
use axum::{http::HeaderMap, routing::get, Json, Router};
use serde_json::json;

#[tokio::test]
async fn follows_port_changes_and_preserves_group_ids_and_model_choices() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::connect(&format!(
        "sqlite://{}",
        dir.path().join("test.db").display()
    ))
    .await
    .unwrap();
    let settings = dir.path().join("settings.json");
    let channel = Magpie::new(store.clone(), settings.clone()).unwrap();
    let gateway = Router::new().route("/api/hello", get(|| async { Json(json!({"name":"magpie"})) }))
        .route("/v1/models", get(|headers: HeaderMap| async move {
            assert_eq!(headers["authorization"], "Bearer magpie-cursor-byok");
            Json(json!({"data":[{"id":"group/work","magpie_label":"Work","context_window":128000,"max_output_tokens":4096}, {"id":"vendor/b"}]}))
        }));
    let listen = async |router| {
        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = socket.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(socket, router).await.unwrap() });
        port
    };
    assert!(store.magpie_catalog().await.unwrap().is_none());
    let first = listen(gateway.clone()).await;
    std::fs::write(&settings, json!({"port":first}).to_string()).unwrap();
    channel.sync(true).await.unwrap();
    channel.select(vec!["group/work".into()]).await.unwrap();
    let before = store.configured_models().await.unwrap().remove(0);
    let second = listen(gateway).await;
    std::fs::write(&settings, json!({"port":second}).to_string()).unwrap();
    channel.sync(false).await.unwrap();
    let models = store.configured_models().await.unwrap();
    assert_eq!(models.len(), 1);
    let after = store
        .configured_model(&before.model_hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after.request_url().unwrap(),
        format!("http://127.0.0.1:{second}/v1/messages")
    );
    assert_eq!(after.model_id, "group/work");
    assert_eq!(after.display_name, "Work");
    assert_eq!(after.context_window_tokens, Some(128000));
    assert_eq!(after.max_output_tokens(), Some(4096));
    let headers =
        |session| crate::provider::router::routed_request_headers(&after, session).unwrap();
    let first_turn = headers("conversation-a");
    assert_eq!(first_turn, headers("conversation-a"));
    let other = headers("conversation-b");
    for key in ["x-session-affinity", "session_id", "x-client-request-id"] {
        assert_eq!(first_turn[key], "conversation-a");
        assert_eq!(other[key], "conversation-b");
    }
    // Models served through an ordinary provider keep their configured headers.
    let mut manual = after.clone();
    manual.model_hash = "byok-manual".into();
    let manual_headers =
        crate::provider::router::routed_request_headers(&manual, "conversation-a").unwrap();
    assert_eq!(manual_headers.len(), 1);
    assert_eq!(manual_headers["user-agent"], "cursor-byok");
    let wrong = listen(Router::new().route(
        "/api/hello",
        get(|| async { Json(json!({"name":"other"})) }),
    ))
    .await;
    std::fs::write(&settings, json!({"port":wrong}).to_string()).unwrap();
    assert!(channel.sync(false).await.is_err());
    assert_eq!(store.magpie_catalog().await.unwrap().unwrap().port, second);
    channel.disconnect().await.unwrap();
    channel.sync(false).await.unwrap();
    assert!(store
        .configured_model(&before.model_hash)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn a_rejected_gateway_records_a_failed_call() {
    use crate::{
        model::{ModelInvocation, ModelRequest, ModelSpec, PromptSpec},
        network::NetworkClients,
        plugin::{PluginRegistry, PluginRuntime},
        provider::{Provider, ProviderRouter},
    };
    use futures_util::StreamExt;
    use tokio_util::sync::CancellationToken;

    let dir = tempfile::tempdir().unwrap();
    let store = Store::connect(&format!(
        "sqlite://{}",
        dir.path().join("test.db").display()
    ))
    .await
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let upstream = Router::new().route(
        "/api/hello",
        get(|| async { Json(json!({"name":"other"})) }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let catalog = Catalog {
        port,
        models: vec![serde_json::from_value(json!({"id":"work"})).unwrap()],
        disabled: HashSet::new(),
        aliases: Default::default(),
        protocols: Default::default(),
        group_name: None,
        image_model: None,
    };
    store.save_magpie_catalog(Some(&catalog)).await.unwrap();
    let model = catalog.configurations().next().unwrap();
    let plugins = PluginRegistry::managed(
        store.clone(),
        PluginRuntime::managed().unwrap(),
        "1.1.0".into(),
    )
    .unwrap();
    let provider = ProviderRouter::new(
        store.clone(),
        plugins,
        NetworkClients::new(store.clone()),
        Duration::from_secs(5),
        Duration::from_secs(5),
    );
    let invocation = ModelInvocation {
        call_id: "gateway-call".into(),
        run_id: "gateway-run".into(),
        conversation_id: "gateway-conversation".into(),
        provider_call_index: 0,
        request: ModelRequest {
            model: ModelSpec::new(&model.model_hash),
            prompt: PromptSpec {
                instructions: String::new(),
                tools: Vec::new(),
            },
            history: Vec::new(),
        },
    };
    let mut stream = provider.stream(invocation, CancellationToken::new());
    assert!(stream.next().await.unwrap().is_err());
    let call = store.llm_call("gateway-call").await.unwrap().unwrap();
    assert_eq!(call.status, "error");
    assert!(call
        .error_message
        .unwrap()
        .contains("is not a Magpie gateway"));
    server.abort();
}

#[tokio::test]
async fn customization_renames_models_and_switches_their_protocols() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::connect(&format!(
        "sqlite://{}",
        dir.path().join("test.db").display()
    ))
    .await
    .unwrap();
    let settings = dir.path().join("settings.json");
    let channel = Magpie::new(store.clone(), settings).unwrap();
    let catalog = Catalog {
        port: 3425,
        models: vec![
            serde_json::from_value(json!({
                "id": "group/work",
                "native_endpoints": ["/v1/messages", "/v1/chat/completions"]
            }))
            .unwrap(),
            serde_json::from_value(json!({
                "id": "group/other",
                "native_endpoints": ["/v1/responses"]
            }))
            .unwrap(),
        ],
        disabled: HashSet::new(),
        aliases: Default::default(),
        protocols: Default::default(),
        group_name: None,
        image_model: None,
    };
    store.save_magpie_catalog(Some(&catalog)).await.unwrap();

    // Without overrides the native endpoint wins: Anthropic for the first,
    // the Responses API for a model that only speaks that.
    let plain: Vec<_> = catalog.configurations().collect();
    assert_eq!(
        plain[0].request_url().unwrap(),
        "http://127.0.0.1:3425/v1/messages"
    );
    assert_eq!(
        plain[1].request_url().unwrap(),
        "http://127.0.0.1:3425/v1/responses"
    );
    assert_eq!(plain[0].display_name, "group/work");
    assert_eq!(plain[0].group_name.as_deref(), Some("Magpie"));

    channel
        .customize(Customize {
            image_model: None,
            group_name: Some("My Gateway".into()),
            aliases: [("group/work".to_owned(), "Renamed".to_owned())]
                .into_iter()
                .collect(),
            protocols: [("group/work".to_owned(), Some(Protocol::OpenAiChat))]
                .into_iter()
                .collect(),
        })
        .await
        .unwrap();
    let catalog = store.magpie_catalog().await.unwrap().unwrap();
    let models: Vec<_> = catalog.configurations().collect();
    assert_eq!(models[0].display_name, "Renamed");
    assert_eq!(models[0].group_name.as_deref(), Some("My Gateway"));
    assert_eq!(
        models[0].request_url().unwrap(),
        "http://127.0.0.1:3425/v1/chat/completions"
    );
    // Clearing the alias and protocol falls back to the gateway's own data.
    channel
        .customize(Customize {
            image_model: None,
            group_name: Some("Magpie".into()),
            aliases: [("group/work".to_owned(), String::new())]
                .into_iter()
                .collect(),
            protocols: [("group/work".to_owned(), None)].into_iter().collect(),
        })
        .await
        .unwrap();
    let catalog = store.magpie_catalog().await.unwrap().unwrap();
    assert!(catalog.aliases.is_empty());
    assert!(catalog.protocols.is_empty());
    assert_eq!(catalog.group_name, None);
    let models: Vec<_> = catalog.configurations().collect();
    assert_eq!(models[0].display_name, "group/work");
    assert_eq!(
        models[0].request_url().unwrap(),
        "http://127.0.0.1:3425/v1/messages"
    );
}

#[tokio::test]
async fn customization_rejects_models_the_gateway_never_served() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::connect(&format!(
        "sqlite://{}",
        dir.path().join("test.db").display()
    ))
    .await
    .unwrap();
    let channel = Magpie::new(store.clone(), dir.path().join("settings.json")).unwrap();
    let catalog = Catalog {
        port: 3425,
        models: vec![serde_json::from_value(json!({"id": "real"})).unwrap()],
        disabled: HashSet::new(),
        aliases: Default::default(),
        protocols: Default::default(),
        group_name: None,
        image_model: None,
    };
    store.save_magpie_catalog(Some(&catalog)).await.unwrap();
    let error = channel
        .customize(Customize {
            image_model: None,
            group_name: None,
            aliases: [("phantom".to_owned(), "Name".to_owned())]
                .into_iter()
                .collect(),
            protocols: Default::default(),
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Magpie has no model phantom"));
    let error = channel
        .customize(Customize {
            image_model: None,
            group_name: None,
            aliases: Default::default(),
            protocols: [("phantom".to_owned(), Some(Protocol::OpenAiChat))]
                .into_iter()
                .collect(),
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Magpie has no model phantom"));
}

#[tokio::test]
async fn the_default_image_model_is_customized_cleared_and_kept_across_syncs() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::connect(&format!(
        "sqlite://{}",
        dir.path().join("test.db").display()
    ))
    .await
    .unwrap();
    let settings = dir.path().join("settings.json");
    let channel = Magpie::new(store.clone(), settings.clone()).unwrap();
    store
        .save_magpie_catalog(Some(&Catalog {
            port: 3425,
            models: vec![serde_json::from_value(json!({"id": "chat"})).unwrap()],
            disabled: HashSet::new(),
            aliases: Default::default(),
            protocols: Default::default(),
            group_name: None,
            image_model: None,
        }))
        .await
        .unwrap();

    channel
        .customize(Customize {
            image_model: Some("gpt-image-1.5".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        store.magpie_catalog().await.unwrap().unwrap().image_model,
        Some("gpt-image-1.5".into())
    );

    // A sync keeps it; customize without the field leaves it alone too.
    let gateway = Router::new()
        .route(
            "/api/hello",
            get(|| async { Json(json!({"name":"magpie"})) }),
        )
        .route(
            "/v1/models",
            get(|| async { Json(json!({"data":[{"id":"chat"}]})) }),
        );
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = socket.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(socket, gateway).await.unwrap() });
    std::fs::write(&settings, json!({"port":port}).to_string()).unwrap();
    channel.sync(true).await.unwrap();
    channel
        .customize(Customize {
            group_name: Some("mine".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        store.magpie_catalog().await.unwrap().unwrap().image_model,
        Some("gpt-image-1.5".into())
    );

    channel
        .customize(Customize {
            image_model: Some("  ".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        store.magpie_catalog().await.unwrap().unwrap().image_model,
        None
    );
}
