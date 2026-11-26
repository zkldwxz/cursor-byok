//! `byok-semantic <absolute workspace root>` serves semantic search over MCP stdio;
//! `byok-semantic search <base64 request>` answers one search and exits.

use std::path::PathBuf;
use std::process::ExitCode;

use byok_semantic::SemanticServer;
use rmcp::ServiceExt;

fn workspace_root(arguments: impl Iterator<Item = String>) -> Result<PathBuf, String> {
    let mut arguments = arguments;
    let Some(argument) = arguments.next() else {
        return Err("usage: byok-semantic <absolute workspace root>".to_string());
    };
    if arguments.next().is_some() {
        return Err(format!(
            "byok-semantic takes exactly one argument (the absolute workspace root); \
             got extra argument(s) after `{argument}`"
        ));
    }
    let root = PathBuf::from(&argument);
    if !root.is_absolute() {
        return Err(format!(
            "workspace root must be an absolute path, got `{argument}`"
        ));
    }
    if !root.exists() {
        return Err(format!("workspace root does not exist: `{argument}`"));
    }
    if !root.is_dir() {
        return Err(format!("workspace root is not a directory: `{argument}`"));
    }
    Ok(root)
}

fn main() -> ExitCode {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    // A workspace root is always absolute, so `search` can never be one.
    if let [command, request] = arguments.as_slice() {
        if command == "search" {
            return search(request);
        }
    }
    serve_workspace()
}

// Runs outside the async runtime: the engine's model client is blocking.
fn search(request: &str) -> ExitCode {
    match byok_semantic::search::run(request) {
        Ok(line) => {
            println!("{line}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("byok-semantic: {error}");
            ExitCode::FAILURE
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn serve_workspace() -> ExitCode {
    let root = match workspace_root(std::env::args().skip(1)) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    if let Err(error) = serve(root).await {
        eprintln!("byok-semantic: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

async fn serve(root: PathBuf) -> Result<(), String> {
    let running = SemanticServer::new(root)
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|error| format!("failed to initialize the MCP session: {error}"))?;
    running
        .waiting()
        .await
        .map_err(|error| format!("MCP session ended with an error: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(arguments: &[&str]) -> Result<PathBuf, String> {
        workspace_root(arguments.iter().map(ToString::to_string))
    }

    #[test]
    fn a_missing_argument_fails_with_usage() {
        let error = run(&[]).unwrap_err();
        assert!(error.contains("usage"), "{error}");
    }

    #[test]
    fn a_relative_workspace_root_is_rejected() {
        let error = run(&["relative/root"]).unwrap_err();
        assert!(error.contains("absolute path"), "{error}");
        assert!(error.contains("relative/root"), "{error}");
    }

    #[test]
    fn a_missing_workspace_root_is_rejected() {
        let error = run(&["/definitely/not/a/workspace"]).unwrap_err();
        assert!(error.contains("does not exist"), "{error}");
        assert!(error.contains("/definitely/not/a/workspace"), "{error}");
    }

    #[test]
    fn a_file_workspace_root_is_rejected() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let argument = file.path().to_string_lossy().to_string();
        let error = run(&[argument.as_str()]).unwrap_err();
        assert!(error.contains("not a directory"), "{error}");
    }

    #[test]
    fn extra_arguments_are_rejected() {
        let error = run(&["/tmp", "/extra"]).unwrap_err();
        assert!(error.contains("exactly one argument"), "{error}");
    }

    #[test]
    fn an_absolute_directory_is_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let argument = dir.path().to_string_lossy().to_string();
        let root = run(&[argument.as_str()]).unwrap();
        assert_eq!(root, dir.path());
    }
}
