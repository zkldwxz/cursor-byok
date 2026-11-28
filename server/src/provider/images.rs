//! Generates images through the Magpie gateway's OpenAI-compatible images API.
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use serde::Deserialize;

use crate::{store::Store, Error, Result};

use super::magpie::local_client;

const GENERATION_TIMEOUT: Duration = Duration::from_secs(180);

/// One generated image: where it was written, and its bytes as base64 for Cursor.
#[derive(Debug)]
pub(crate) struct GeneratedImage {
    pub file_path: String,
    pub image_data: String,
}

/// What an approved GenerateImage call asks for, straight out of its arguments.
pub(crate) struct ImageRequest {
    pub description: String,
    pub file_path: Option<String>,
    pub aspect_ratio: Option<String>,
}

#[derive(Clone)]
pub(crate) struct ImageGeneration {
    store: Store,
}

impl ImageGeneration {
    pub(crate) fn new(store: Store) -> Self {
        Self { store }
    }

    /// Runs the channel's configured image model; errors are tool errors, so
    /// the model reads them and can go on with the turn.
    pub(crate) async fn generate(
        &self,
        request: &ImageRequest,
        project: &Path,
    ) -> std::result::Result<GeneratedImage, String> {
        self.generate_inner(request, project)
            .await
            .map_err(|error| error.to_string())
    }

    async fn generate_inner(
        &self,
        request: &ImageRequest,
        project: &Path,
    ) -> Result<GeneratedImage> {
        if request.description.trim().is_empty() {
            return Err(Error::Protocol(
                "GenerateImage is missing description".into(),
            ));
        }
        let catalog = self
            .store
            .magpie_catalog()
            .await?
            .ok_or_else(|| Error::Config("no image channel is connected".into()))?;
        let model = catalog
            .image_model
            .as_deref()
            .ok_or_else(|| {
                Error::Config("no default image model is configured for the channel".into())
            })?
            .to_owned();
        let base = format!("http://127.0.0.1:{}", catalog.port);
        let client = local_client(GENERATION_TIMEOUT)?;
        let mut body = serde_json::json!({
            "model": model,
            "prompt": request.description,
        });
        // The API only takes these sizes; anything else of the model's wording is its own.
        if let Some(size) = request
            .aspect_ratio
            .as_deref()
            .and_then(size_of_aspect_ratio)
        {
            body["size"] = size.into();
        }
        let response = client
            .post(format!("{base}/v1/images/generations"))
            .bearer_auth(super::magpie::app_key())
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            return Err(Error::Provider(format!(
                "image generation failed ({status}): {}",
                text.chars().take(500).collect::<String>()
            )));
        }
        #[derive(Deserialize)]
        struct Reply {
            data: Vec<Image>,
        }
        #[derive(Deserialize)]
        struct Image {
            b64_json: Option<String>,
            url: Option<String>,
        }
        let reply: Reply = serde_json::from_str(&text)
            .map_err(|error| Error::Provider(format!("image generation reply: {error}")))?;
        let image = reply
            .data
            .into_iter()
            .next()
            .ok_or_else(|| Error::Provider("image generation returned no image".into()))?;
        use base64::{engine::general_purpose::STANDARD, Engine};
        let (bytes, image_data) = match (image.b64_json, image.url) {
            (Some(encoded), _) => {
                let bytes = STANDARD.decode(&encoded).map_err(|error| {
                    Error::Provider(format!("image data is not base64: {error}"))
                })?;
                (bytes, encoded)
            }
            (None, Some(url)) => {
                let bytes = client.get(&url).send().await?.bytes().await?.to_vec();
                let encoded = STANDARD.encode(&bytes);
                (bytes, encoded)
            }
            (None, None) => {
                return Err(Error::Provider(
                    "image generation returned neither data nor a URL".into(),
                ))
            }
        };
        let path = file_path(request.file_path.as_deref(), project);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, &bytes).await?;
        Ok(GeneratedImage {
            file_path: path.to_string_lossy().into_owned(),
            image_data,
        })
    }
}

/// OpenAI's sizes, as the aspect ratio the model asked for maps to them.
fn size_of_aspect_ratio(aspect_ratio: &str) -> Option<&'static str> {
    match aspect_ratio.trim() {
        "1:1" => Some("1024x1024"),
        "3:2" => Some("1536x1024"),
        "2:3" => Some("1024x1536"),
        _ => None,
    }
}

/// Where the image goes: the path the model named, clamped inside the
/// project — a leading root and `..` parts fall away, so no approval for one
/// file writes another; a generated name beside it when it named none.
fn file_path(named: Option<&str>, project: &Path) -> PathBuf {
    let named = named.map(str::trim).filter(|named| !named.is_empty());
    match named {
        Some(named) => {
            use std::path::Component;
            let mut path = project.to_path_buf();
            for component in Path::new(named).components() {
                match component {
                    Component::Normal(part) => path.push(part),
                    Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
                    Component::ParentDir => {
                        path.pop();
                        if !path.starts_with(project) {
                            path = project.to_path_buf();
                        }
                    }
                }
            }
            path
        }
        None => project.join(format!("generated-{}.png", uuid::Uuid::new_v4().simple())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::State, routing::post, Json, Router};
    use serde_json::{json, Value};

    #[test]
    fn aspect_ratios_map_to_openai_sizes_and_unknown_ones_are_the_models_own() {
        assert_eq!(size_of_aspect_ratio("1:1"), Some("1024x1024"));
        assert_eq!(size_of_aspect_ratio("3:2"), Some("1536x1024"));
        assert_eq!(size_of_aspect_ratio("2:3"), Some("1024x1536"));
        assert_eq!(size_of_aspect_ratio("16:9"), None);
        assert_eq!(size_of_aspect_ratio(""), None);
    }

    #[test]
    fn the_model_chosen_path_resolves_inside_the_project() {
        let project = Path::new("/work/project");
        assert_eq!(
            file_path(Some("assets/hero.png"), project),
            PathBuf::from("/work/project/assets/hero.png")
        );
        assert_eq!(
            file_path(Some("  "), project).parent().unwrap(),
            Path::new("/work/project")
        );
    }

    #[test]
    fn no_named_path_writes_outside_the_project() {
        let project = Path::new("/work/project");
        for (named, expected) in [
            ("/tmp/abs.png", "/work/project/tmp/abs.png"),
            ("../escape.png", "/work/project/escape.png"),
            ("../../etc/cron.d/x", "/work/project/etc/cron.d/x"),
            ("out/../../back.png", "/work/project/back.png"),
        ] {
            assert_eq!(
                file_path(Some(named), project),
                PathBuf::from(expected),
                "{named}"
            );
        }
    }

    async fn store() -> Store {
        let directory = tempfile::tempdir().unwrap().keep();
        Store::connect(&format!(
            "sqlite://{}",
            directory.join("state.db").display()
        ))
        .await
        .unwrap()
    }

    async fn gateway(body: Value) -> (u16, tokio::sync::mpsc::UnboundedReceiver<Value>) {
        let (sent, requests) = tokio::sync::mpsc::unbounded_channel();
        let router = Router::new().route(
            "/v1/images/generations",
            post(move |State(()): State<()>, Json(request): Json<Value>| {
                let sent = sent.clone();
                let body = body.clone();
                async move {
                    sent.send(request).unwrap();
                    Json(body)
                }
            }),
        );
        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = socket.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(socket, router).await.unwrap() });
        (port, requests)
    }

    #[tokio::test]
    async fn an_approved_generation_writes_the_image_where_the_model_named_it() {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let pixels = b"png-bytes";
        let (port, mut requests) = gateway(json!({
            "data": [{"b64_json": STANDARD.encode(pixels)}]
        }))
        .await;
        let store = store().await;
        let catalog = crate::provider::magpie::test_catalog(port, "gpt-image-1.5");
        store.save_magpie_catalog(Some(&catalog)).await.unwrap();
        let project = tempfile::tempdir().unwrap();
        let generation = ImageGeneration::new(store);
        let image = generation
            .generate(
                &ImageRequest {
                    description: "a crane in the fog".into(),
                    file_path: Some("out/crane.png".into()),
                    aspect_ratio: Some("3:2".into()),
                },
                project.path(),
            )
            .await
            .unwrap();
        let written = project.path().join("out/crane.png");
        assert_eq!(image.file_path, written.to_string_lossy());
        assert_eq!(std::fs::read(&written).unwrap(), pixels);
        assert_eq!(image.image_data, STANDARD.encode(pixels));
        let request = requests.try_recv().unwrap();
        assert_eq!(request["model"], "gpt-image-1.5");
        assert_eq!(request["prompt"], "a crane in the fog");
        assert_eq!(request["size"], "1536x1024");
    }

    #[tokio::test]
    async fn without_a_configured_image_model_the_tool_error_says_so() {
        let store = store().await;
        let catalog = crate::provider::magpie::test_catalog(1, "");
        store.save_magpie_catalog(Some(&catalog)).await.unwrap();
        let error = ImageGeneration::new(store)
            .generate(
                &ImageRequest {
                    description: "anything".into(),
                    file_path: None,
                    aspect_ratio: None,
                },
                Path::new("/tmp"),
            )
            .await
            .unwrap_err();
        assert!(error.contains("no default image model"), "{error}");
    }

    #[tokio::test]
    async fn a_gateway_failure_is_the_tools_error_not_the_runs() {
        let (port, _requests) = gateway(json!({"error": {"message": "model unavailable"}})).await;
        // Answer 200 or not, a wrong body shape is a provider error too; here
        // the data list is missing.
        let store = store().await;
        let catalog = crate::provider::magpie::test_catalog(port, "gpt-image-1.5");
        store.save_magpie_catalog(Some(&catalog)).await.unwrap();
        let error = ImageGeneration::new(store)
            .generate(
                &ImageRequest {
                    description: "anything".into(),
                    file_path: None,
                    aspect_ratio: None,
                },
                Path::new("/tmp"),
            )
            .await
            .unwrap_err();
        assert!(!error.is_empty());
    }
}
