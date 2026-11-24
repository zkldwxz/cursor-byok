//! Runs a temporary Cloudflare tunnel to the phone listener.
use std::{net::SocketAddr, path::PathBuf, process::Stdio, sync::LazyLock, time::Duration};

use regex::Regex;
use tokio::{
    io::{AsyncBufReadExt, BufReader, Lines},
    process::{Child, ChildStderr, Command},
};

use crate::{Error, Result};

const START_TIMEOUT: Duration = Duration::from_secs(40);

pub(super) struct Tunnel {
    pub url: String,
    child: Child,
    log: Lines<BufReader<ChildStderr>>,
}

/// GUI apps do not inherit the shell's PATH on macOS.
pub(super) fn find_cloudflared() -> Option<PathBuf> {
    let name = format!("cloudflared{}", std::env::consts::EXE_SUFFIX);
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(["/opt/homebrew/bin".into(), "/usr/local/bin".into()])
        .chain(
            dirs::home_dir()
                .into_iter()
                .flat_map(|home| [home.join("bin"), home.join(".local/bin")]),
        )
        .map(|directory| directory.join(&name))
        .find(|candidate| candidate.is_file())
}

impl Tunnel {
    pub async fn start(program: &std::path::Path, target: SocketAddr) -> Result<Self> {
        let mut child = Command::new(program)
            .args(["tunnel", "--no-autoupdate", "--url"])
            .arg(format!("http://{target}"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| {
                Error::Config(format!("cannot start {}: {error}", program.display()))
            })?;
        let stderr = child.stderr.take().expect("cloudflared stderr is piped");
        let mut log = BufReader::new(stderr).lines();
        let mut last = String::new();
        let found = tokio::time::timeout(START_TIMEOUT, async {
            while let Some(line) = log.next_line().await? {
                if let Some(url) = public_url(&line) {
                    return Ok(Some(url));
                }
                last = line;
            }
            Ok::<_, Error>(None)
        })
        .await
        .map_err(|_| Error::Config("cloudflared did not open a tunnel within 40 seconds".into()))?;
        match found? {
            Some(url) => Ok(Self { url, child, log }),
            None => Err(Error::Config(format!(
                "cloudflared exited before opening a tunnel: {last}"
            ))),
        }
    }

    /// Returns once cloudflared has gone. Its log is drained meanwhile, since a
    /// full pipe would block it.
    pub async fn closed(&mut self) {
        while let Ok(Some(line)) = self.log.next_line().await {
            tracing::debug!(target: "cloudflared", "{line}");
        }
        let _ = self.child.wait().await;
    }
}

fn public_url(line: &str) -> Option<String> {
    static URL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"https://([a-z0-9-]+)\.trycloudflare\.com").unwrap());
    // Failures mention the service's own address.
    URL.captures_iter(line)
        .find(|found| &found[1] != "api")
        .map(|found| found[0].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_tunnels_own_address_counts_as_open() {
        assert_eq!(
            public_url("INF |  https://quick-brown-fox.trycloudflare.com   |").as_deref(),
            Some("https://quick-brown-fox.trycloudflare.com")
        );
        assert_eq!(
            public_url("ERR Post \"https://api.trycloudflare.com/tunnel\": EOF"),
            None
        );
        assert_eq!(public_url("INF Requesting new quick Tunnel"), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_tunnel_that_cannot_open_reports_why() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let program = directory.path().join("cloudflared");
        std::fs::write(&program, "#!/bin/sh\necho 'ERR no route to edge' >&2\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let error = Tunnel::start(&program, "127.0.0.1:1".parse().unwrap())
            .await
            .err()
            .unwrap();
        assert!(error.to_string().contains("no route to edge"), "{error}");
    }
}
