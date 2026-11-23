//! Removes bundled plugins that earlier releases pre-installed into the user's installed directory.
use std::path::Path;

use crate::Result;

/// Plugin ids this app no longer bundles. Earlier releases wrote them into
/// `plugins/installed`; a copy left behind would keep loading, so startup
/// deletes exactly those directories and never a user-authored plugin.
const RETIRED_IDS: &[&str] = &[
    "dev.cursorbyok.plugins.zcode",
    "dev.cursorbyok.examples.codex-auth",
    "dev.cursorbyok.examples.grok-auth",
    "dev.cursorbyok.plugins.antigravity-auth",
];

/// Directory names the retired plugins used; a missing directory is normal.
const RETIRED_DIRECTORIES: &[&str] = &["zcode-auth", "codex-auth", "grok-auth", "antigravity-auth"];

pub(super) fn install(installed: &Path) -> Result<()> {
    for name in RETIRED_DIRECTORIES {
        let directory = installed.join(name);
        let manifest = directory.join("plugin.json");
        let is_retired = match std::fs::read(&manifest) {
            Ok(bytes) => match serde_json::from_slice::<serde_json::Value>(&bytes) {
                Ok(value) => value["id"]
                    .as_str()
                    .is_some_and(|id| RETIRED_IDS.contains(&id)),
                Err(error) => {
                    tracing::warn!(path = %manifest.display(), %error, "cannot identify retired plugin");
                    false
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => {
                tracing::warn!(path = %manifest.display(), %error, "cannot read retired plugin manifest");
                false
            }
        };
        if is_retired {
            std::fs::remove_dir_all(directory)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retiring_removes_each_bundled_plugin_from_an_existing_install() {
        let root = tempfile::tempdir().unwrap();
        let manifests = [
            ("codex-auth", "dev.cursorbyok.examples.codex-auth"),
            ("grok-auth", "dev.cursorbyok.examples.grok-auth"),
            (
                "antigravity-auth",
                "dev.cursorbyok.plugins.antigravity-auth",
            ),
            ("zcode-auth", "dev.cursorbyok.plugins.zcode"),
        ];
        for (directory, id) in manifests {
            let path = root.path().join(directory);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(
                path.join("plugin.json"),
                format!(r#"{{"id":"{id}","version":"0.1.0"}}"#),
            )
            .unwrap();
        }
        install(root.path()).unwrap();
        for (directory, _) in manifests {
            assert!(
                !root.path().join(directory).exists(),
                "{directory} survived"
            );
        }
    }

    #[test]
    fn a_same_named_user_plugin_is_kept() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("codex-auth");
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("plugin.json"), r#"{"id":"my.plugin"}"#).unwrap();
        install(root.path()).unwrap();
        assert!(path.join("plugin.json").is_file());
    }

    #[test]
    fn an_invalid_manifest_does_not_block_the_other_retirements() {
        let root = tempfile::tempdir().unwrap();
        let broken = root.path().join("codex-auth");
        std::fs::create_dir(&broken).unwrap();
        std::fs::write(broken.join("plugin.json"), "{").unwrap();
        let grok = root.path().join("grok-auth");
        std::fs::create_dir(&grok).unwrap();
        std::fs::write(
            grok.join("plugin.json"),
            r#"{"id":"dev.cursorbyok.examples.grok-auth"}"#,
        )
        .unwrap();

        install(root.path()).unwrap();

        assert_eq!(
            std::fs::read_to_string(broken.join("plugin.json")).unwrap(),
            "{"
        );
        assert!(!grok.exists());
    }
}
