//! One search per process: a base64 JSON request in, one JSON line out.

use std::path::PathBuf;

use base64::{engine::general_purpose::STANDARD, Engine};
use semble_core::{SearchEngine, SearchRequest, SembleConfig};
use serde::{Deserialize, Serialize};

// A minified line would otherwise push one snippet past what Cursor keeps of a
// command's output, and a cut-off line is no longer JSON.
const MAX_SNIPPET_CHARS: usize = 2_000;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub query: String,
    pub root: PathBuf,
    pub top_k: usize,
}

#[derive(Debug, Serialize)]
pub struct Response {
    pub results: Vec<Location>,
}

#[derive(Debug, Serialize)]
pub struct Location {
    pub path: PathBuf,
    pub start_line: usize,
    pub end_line: usize,
    pub score: f32,
    pub content: String,
}

/// The request is base64 because it travels through the user's login shell.
pub fn decode(argument: &str) -> Result<Request, String> {
    let json = STANDARD
        .decode(argument)
        .map_err(|error| format!("search request is not base64: {error}"))?;
    let request: Request = serde_json::from_slice(&json)
        .map_err(|error| format!("search request is not valid JSON: {error}"))?;
    if !request.root.is_absolute() {
        return Err(format!(
            "search root must be an absolute path, got `{}`",
            request.root.display()
        ));
    }
    if !request.root.is_dir() {
        return Err(format!(
            "search root is not a directory: `{}`",
            request.root.display()
        ));
    }
    Ok(request)
}

pub fn run(argument: &str) -> Result<String, String> {
    let request = decode(argument)?;
    let engine = SearchEngine::load_default(SembleConfig::default())
        .map_err(|error| format!("failed to load the semantic engine: {error}"))?;
    search(&engine, request)
}

pub fn search(engine: &SearchEngine, request: Request) -> Result<String, String> {
    let mut query = SearchRequest::new(request.query, &request.root);
    query.top_k = request.top_k;
    let found = engine.search(query).map_err(|error| error.to_string())?;
    let results = found
        .results
        .into_iter()
        .map(|result| Location {
            path: request.root.join(result.file_path),
            start_line: result.start_line,
            end_line: result.end_line,
            score: result.score,
            content: result
                .content
                .unwrap_or_default()
                .chars()
                .take(MAX_SNIPPET_CHARS)
                .collect(),
        })
        .collect();
    serde_json::to_string(&Response { results }).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::Arc};

    use semble_core::Embedder;

    use super::*;

    struct LengthEmbedder;

    impl Embedder for LengthEmbedder {
        fn id(&self) -> &str {
            "length-v1"
        }

        fn dimensions(&self) -> usize {
            1
        }

        fn encode(&self, texts: &[String]) -> semble_core::Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![1.0]).collect())
        }
    }

    fn encoded(value: serde_json::Value) -> String {
        STANDARD.encode(value.to_string())
    }

    fn workspace(body: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("auth.rs"), body).unwrap();
        dir
    }

    fn found(body: &str) -> (tempfile::TempDir, serde_json::Value) {
        let source = workspace(body);
        let cache = tempfile::tempdir().unwrap();
        let engine =
            SearchEngine::with_embedder(SembleConfig::new(cache.path()), Arc::new(LengthEmbedder));
        let request = decode(&encoded(serde_json::json!({
            "query": "verify token", "root": source.path(), "top_k": 1
        })))
        .unwrap();
        let line = search(&engine, request).unwrap();
        assert!(!line.contains('\n'), "{line}");
        (source, serde_json::from_str(&line).unwrap())
    }

    #[test]
    fn a_result_names_an_absolute_path_and_one_based_lines() {
        let (source, response) = found("fn authenticate() {\n    verify_token();\n}\n");
        let first = &response["results"][0];
        assert_eq!(
            first["path"].as_str().unwrap(),
            source.path().join("auth.rs").to_str().unwrap()
        );
        assert_eq!(first["start_line"], 1);
        assert!(first["content"].as_str().unwrap().contains("verify_token"));
    }

    #[test]
    fn one_very_long_line_does_not_grow_the_output_without_bound() {
        let (_source, response) = found(&format!(
            "fn verify_token() {{ let x = \"{}\"; }}\n",
            "y".repeat(50_000)
        ));
        for result in response["results"].as_array().unwrap() {
            assert!(result["content"].as_str().unwrap().chars().count() <= MAX_SNIPPET_CHARS);
        }
    }

    #[test]
    fn malformed_requests_name_what_is_wrong() {
        assert!(decode("%%%").unwrap_err().contains("not base64"));
        assert!(decode(&STANDARD.encode("{"))
            .unwrap_err()
            .contains("not valid JSON"));
        let relative = encoded(serde_json::json!({"query": "a", "root": "src", "top_k": 1}));
        assert!(decode(&relative).unwrap_err().contains("absolute path"));
        let gone = tempfile::tempdir().unwrap().path().join("gone");
        let missing = encoded(serde_json::json!({"query": "a", "root": gone, "top_k": 1}));
        assert!(decode(&missing).unwrap_err().contains("not a directory"));
    }
}
