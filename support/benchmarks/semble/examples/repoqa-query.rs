//! Exposes Semble's real indexing and search APIs to the RepoQA benchmark.

use std::{
    env,
    io::{self, BufRead, Write},
    path::PathBuf,
    time::Instant,
};

use semble_core::{ContentType, SearchEngine, SearchRequest, SembleConfig};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
struct Query {
    root: PathBuf,
    query: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args_os().skip(1);
    let cache = args.next().ok_or("expected index cache directory")?;
    let engine = SearchEngine::load_default(SembleConfig::new(cache))?;
    let mut indexes = Vec::new();
    for root in args.map(PathBuf::from) {
        let started = Instant::now();
        let stats = engine.prepare(&root, &[ContentType::Code])?;
        indexes.push(json!({
            "root": root,
            "ms": started.elapsed().as_secs_f64() * 1000.0,
            "files": stats.file_count,
            "chunks": stats.chunk_count,
        }));
    }
    let mut out = io::stdout().lock();
    writeln!(out, "{}", json!(indexes))?;
    out.flush()?;

    for line in io::stdin().lock().lines() {
        let query: Query = serde_json::from_str(&line?)?;
        let mut request = SearchRequest::new(query.query, query.root);
        request.top_k = 10;
        request.max_snippet_lines = Some(0);
        let started = Instant::now();
        let result = engine.search(request)?;
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        let hits = result
            .results
            .into_iter()
            .map(|hit| (hit.file_path, hit.start_line, hit.end_line))
            .collect::<Vec<_>>();
        writeln!(out, "{}", json!({ "ms": elapsed_ms, "hits": hits }))?;
        out.flush()?;
    }
    Ok(())
}
