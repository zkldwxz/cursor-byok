//! Terminates the Cursor desktop process before an explicit takeover.

use tokio::process::Command;

use crate::{Error, Result};

pub async fn terminate_cursor() -> Result<()> {
    terminate_platform_cursor().await
}

#[cfg(target_os = "macos")]
async fn terminate_platform_cursor() -> Result<()> {
    terminate_unix_process("Cursor").await
}

#[cfg(target_os = "linux")]
async fn terminate_platform_cursor() -> Result<()> {
    terminate_unix_process("cursor").await?;
    terminate_unix_process("Cursor").await
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
async fn terminate_unix_process(name: &str) -> Result<()> {
    let running = Command::new("pgrep").args(["-x", name]).status().await?;
    if !running.success() {
        return match running.code() {
            Some(1) => Ok(()),
            _ => Err(Error::Config(format!(
                "failed to inspect the {name} process"
            ))),
        };
    }
    let terminated = Command::new("pkill").args(["-x", name]).status().await?;
    if terminated.success() || terminated.code() == Some(1) {
        Ok(())
    } else {
        Err(Error::Config(format!(
            "failed to terminate the {name} process"
        )))
    }
}

#[cfg(target_os = "windows")]
async fn terminate_platform_cursor() -> Result<()> {
    let processes = Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq Cursor.exe", "/NH", "/FO", "CSV"])
        .output()
        .await?;
    if !processes.status.success() {
        return Err(Error::Config(
            "failed to inspect the Cursor.exe process".into(),
        ));
    }
    if !String::from_utf8_lossy(&processes.stdout)
        .to_ascii_lowercase()
        .contains("cursor.exe")
    {
        return Ok(());
    }
    let terminated = Command::new("taskkill")
        .args(["/F", "/T", "/IM", "Cursor.exe"])
        .status()
        .await?;
    if terminated.success() {
        Ok(())
    } else {
        Err(Error::Config(
            "failed to terminate the Cursor.exe process".into(),
        ))
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
async fn terminate_platform_cursor() -> Result<()> {
    Err(Error::Config(format!(
        "terminating Cursor is unsupported on {}",
        std::env::consts::OS
    )))
}
