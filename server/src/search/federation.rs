//! Aggregates and ranks results from search sources.
use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashMap},
    time::Duration,
};

use futures_util::{stream::FuturesUnordered, StreamExt};

use crate::store::Store;

use super::{catalog, SearchEngine, SearchHit};

const RRF_K: f64 = 60.0;
const MAX_RESULTS: usize = 15;
const SEARCH_DEADLINE: Duration = Duration::from_secs(4);

#[derive(Clone)]
pub struct WebSearch {
    client: SearchClient,
    engines: Vec<SearchEngine>,
}

#[derive(Clone)]
enum SearchClient {
    Managed(Store),
    Direct(reqwest::Client),
}

#[derive(Debug, thiserror::Error)]
#[error("web search failed: {0}")]
pub struct SearchError(String);

impl WebSearch {
    pub fn built_in() -> Self {
        Self::with_engines(catalog::engines())
    }

    pub(crate) fn managed(store: Store) -> Self {
        Self {
            client: SearchClient::Managed(store),
            engines: catalog::engines(),
        }
    }

    pub fn with_engines<I, E>(engines: I) -> Self
    where
        I: IntoIterator<Item = E>,
        E: Into<SearchEngine>,
    {
        Self {
            client: SearchClient::Direct(reqwest::Client::new()),
            engines: engines.into_iter().map(Into::into).collect(),
        }
    }

    pub fn engine_ids(&self) -> Vec<&'static str> {
        self.engines.iter().map(SearchEngine::id).collect()
    }

    pub async fn search(&self, query: &str) -> Result<Vec<SearchHit>, SearchError> {
        let query = query.trim();
        if query.is_empty() {
            return Err(SearchError("query is empty".into()));
        }
        let client = match &self.client {
            SearchClient::Managed(store) => crate::network::client(store)
                .await
                .map_err(|error| SearchError(format!("HTTP client failed: {error}")))?,
            SearchClient::Direct(client) => client.clone(),
        };
        let mut pending: FuturesUnordered<_> = self
            .engines
            .iter()
            .enumerate()
            .map(|(index, engine)| {
                let client = &client;
                async move { (index, engine.search(client, query).await) }
            })
            .collect();
        let mut responses = BTreeMap::new();
        // Keep configured ranking order, independent of network arrival order.
        let deadline = tokio::time::Instant::now() + SEARCH_DEADLINE;
        while let Ok(Some((index, response))) =
            tokio::time::timeout_at(deadline, pending.next()).await
        {
            responses.insert(index, response);
        }
        drop(pending);
        let mut merged = HashMap::<String, SearchHit>::new();
        let mut failures = Vec::new();
        for (index, engine) in self.engines.iter().enumerate() {
            match responses.remove(&index) {
                Some(Ok(results)) => merge(&mut merged, engine.id(), results),
                Some(Err(error)) => {
                    tracing::warn!(engine = engine.id(), %error, "search engine failed");
                    failures.push(engine.id());
                }
                None => {
                    tracing::debug!(engine = engine.id(), "search deadline reached");
                    failures.push(engine.id());
                }
            }
        }
        if !failures.is_empty() && failures.len() == self.engines.len() {
            return Err(SearchError(format!(
                "no usable results; these engines failed or timed out: {}",
                failures.join(", ")
            )));
        }
        let mut results = merged.into_values().collect::<Vec<_>>();
        let phrases = super::query::named_phrases(query);
        if phrases.len() > 1 {
            let phrases = phrases
                .iter()
                .map(|name| name.to_lowercase())
                .collect::<Vec<_>>();
            for hit in &mut results {
                let title = hit.title.to_lowercase();
                // A directly named subject is stronger than one single-source
                // RRF vote, but does not outweigh independent agreement.
                if phrases.iter().any(|phrase| title.contains(phrase)) {
                    hit.score *= 1.5;
                }
            }
        }
        results.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.url.cmp(&right.url))
        });
        results.truncate(MAX_RESULTS);
        Ok(results)
    }
}

impl Default for WebSearch {
    fn default() -> Self {
        Self::built_in()
    }
}

fn merge(merged: &mut HashMap<String, SearchHit>, engine: &'static str, results: Vec<SearchHit>) {
    for (rank, mut result) in results.into_iter().enumerate() {
        let score = 1.0 / (RRF_K + rank as f64 + 1.0);
        match merged.get_mut(&result.url) {
            Some(existing) => {
                // Reciprocal Rank Fusion sums one term per ranked list. One
                // engine listing a URL more than once is still one list, so
                // only its best rank counts; a later repeat contributes
                // nothing but its snippet.
                if !existing.engines.contains(&engine) {
                    existing.score += score;
                    existing.engines.push(engine);
                }
                if result.chunk.len() > existing.chunk.len() {
                    existing.chunk = result.chunk;
                }
            }
            None => {
                result.score = score;
                merged.insert(result.url.clone(), result);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_slow_search_engine_does_not_block_useful_results_and_empty_is_not_an_error() {
        use axum::{routing::get, Router};
        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        let app = Router::new().route("/fast", get(|| async { r#"<div class='res'><a href='https://example.com/result'>Result</a><span>Snippet</span></div>"# }))
            .route("/empty",get(|| async { "<html>No matching documents</html>" }))
            .route("/failed",get(|| async { axum::http::StatusCode::BAD_GATEWAY }))
            .route("/slow",get(|| async { std::future::pending::<&'static str>().await }));
        let server = tokio::spawn(async move { axum::serve(socket, app).await.unwrap() });
        let make = |id, path| {
            super::super::HtmlEngine::new(
                id,
                format!("http://{addr}/{path}?q={{query}}"),
                ".res",
                "a",
                "a",
                "span",
            )
        };
        let use_partial = WebSearch::with_engines([make("fast", "fast"), make("slow", "slow")]);
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(6),
            use_partial.search("test"),
        )
        .await
        .expect("slow source cannot consume its whole HTTP timeout")
        .unwrap();
        assert_eq!(result[0].url, "https://example.com/result");
        assert!(WebSearch::with_engines([make("empty", "empty")])
            .search("test")
            .await
            .unwrap()
            .is_empty());
        assert!(
            WebSearch::with_engines([make("empty", "empty"), make("failed", "failed")])
                .search("test")
                .await
                .unwrap()
                .is_empty()
        );
        assert!(WebSearch::with_engines([make("failed", "failed")])
            .search("test")
            .await
            .is_err());
        server.abort();
    }

    #[tokio::test]
    async fn search_unifies_tracking_links_but_preserves_meaningful_query_parameters() {
        use axum::{extract::Query, routing::get, Router};

        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        let app = Router::new()
            .route("/first", get(|headers: axum::http::HeaderMap, Query(params): Query<HashMap<String, String>>| async move {
                assert_eq!(headers["user-agent"], "PublicSearchClient/1.0");
                assert_eq!(params.get("q").map(String::as_str), Some("MCP + cache & 中文"));
                r#"<div class='res'><a href='https://example.com/page?id=2&amp;utm_source=one#details'>Reference</a><span>Short</span></div>"#
            }))
            .route("/second", get(|| async {
                r#"<div class='res'><a href='https://example.com/page?id=2&amp;gclid=abc'>Reference</a><span>A longer reference excerpt</span></div>
                   <div class='res'><a href='https://example.com/page?id=3'>Other</a><span>Other page</span></div>"#
            }));
        let server = tokio::spawn(async move { axum::serve(socket, app).await.unwrap() });
        let search = WebSearch::with_engines(["first", "second"].map(|id| {
            super::super::HtmlEngine::new(
                id,
                format!("http://{addr}/{id}?q={{query}}"),
                ".res",
                "a",
                "a",
                "span",
            )
        }));

        let hits = search.search("MCP + cache & 中文").await.unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].url, "https://example.com/page?id=2");
        assert_eq!(hits[0].engines, ["first", "second"]);
        assert_eq!(hits[0].chunk, "A longer reference excerpt");
        assert_eq!(hits[1].url, "https://example.com/page?id=3");
        server.abort();
    }

    #[tokio::test]
    async fn compound_query_recovers_named_articles_when_the_reference_search_is_empty() {
        use axum::{extract::Query, routing::get, Router};
        use serde_json::json;

        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        let app = Router::new()
            .route("/reference", get(|Query(params): Query<HashMap<String, String>>| async move {
                let titles = if params["q"] == "\"Marie Curie\" OR \"Niels Bohr\"" {
                    vec![json!({"title":"Marie Curie","url":"https://example.com/marie-curie"}),
                         json!({"title":"Niels Bohr","url":"https://example.com/niels-bohr"})]
                } else {
                    vec![]
                };
                axum::Json(json!({"items": titles}))
            }))
            .route("/general", get(|| async {
                axum::Json(json!({"items":[
                    {"title":"Physics awards and prizes", "url":"https://aaa.example.com/prizes"},
                    {"title":"Marie Curie", "url":"https://example.com/marie-curie?utm_source=general"}
                ]}))
            }));
        let server = tokio::spawn(async move { axum::serve(socket, app).await.unwrap() });
        let engine = |id, path| {
            super::super::JsonEngine::new(
                id,
                format!("http://{addr}/{path}?q={{query}}"),
                "/items",
                "/title",
                "/url",
                "/snippet",
                None,
            )
        };
        let search = WebSearch::with_engines([
            engine("general", "general"),
            engine("wikipedia", "reference"),
        ]);

        let hits = search
            .search("Marie Curie first prize and Niels Bohr second prize")
            .await
            .unwrap();
        assert_eq!(hits[0].title, "Marie Curie");
        assert_eq!(hits[0].engines, ["general", "wikipedia"]);
        assert_eq!(hits[1].title, "Niels Bohr");
        server.abort();
    }

    fn hit(url: &str, engine: &'static str, chunk: &str) -> SearchHit {
        SearchHit::new("title", url, chunk, vec![engine])
    }

    fn rrf(rank: usize) -> f64 {
        1.0 / (RRF_K + rank as f64 + 1.0)
    }

    #[test]
    fn an_engine_that_lists_one_url_twice_contributes_a_single_rrf_term() {
        let mut merged = HashMap::new();
        merge(
            &mut merged,
            "duckduckgo",
            vec![
                hit("https://example.com/p", "duckduckgo", "short"),
                hit("https://example.com/p", "duckduckgo", "longer snippet"),
            ],
        );

        let entry = &merged["https://example.com/p"];
        assert_eq!(entry.engines, vec!["duckduckgo"]);
        assert_eq!(entry.score, rrf(0));
        assert_eq!(entry.chunk, "longer snippet");
    }

    #[test]
    fn two_engines_that_agree_on_a_url_still_sum_both_ranks() {
        let mut merged = HashMap::new();
        merge(
            &mut merged,
            "google",
            vec![hit("https://example.com/p", "google", "a")],
        );
        merge(
            &mut merged,
            "bing",
            vec![
                hit("https://example.com/other", "bing", "b"),
                hit("https://example.com/p", "bing", "c"),
            ],
        );

        let entry = &merged["https://example.com/p"];
        assert_eq!(entry.engines, vec!["google", "bing"]);
        assert_eq!(entry.score, rrf(0) + rrf(1));
    }

    #[test]
    fn agreement_between_engines_outranks_one_engine_repeating_itself() {
        let mut merged = HashMap::new();
        merge(
            &mut merged,
            "duckduckgo",
            vec![
                hit("https://example.com/repeated", "duckduckgo", "a"),
                hit("https://example.com/repeated", "duckduckgo", "b"),
                hit("https://example.com/agreed", "duckduckgo", "c"),
            ],
        );
        merge(
            &mut merged,
            "brave",
            vec![hit("https://example.com/agreed", "brave", "d")],
        );

        assert!(
            merged["https://example.com/agreed"].score
                > merged["https://example.com/repeated"].score
        );
    }
}
