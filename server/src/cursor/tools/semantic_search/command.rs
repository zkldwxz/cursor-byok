//! Builds the fixed shell commands of a semantic search and reads what they report.

use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::json;

/// The program or the model is not on the machine that holds the code.
pub(super) const MISSING: u32 = 97;

/// Every value reaches the script as a quoted argument; nothing is spliced into
/// it. The script holds no quote and no backslash, the two characters login
/// shells disagree about inside single quotes.
const SCRIPT: &str = r#"
home="$HOME/.cursor-byok-v3"
program="$home/semantic/$1/byok-semantic"
cache="$home/cache/semble"
model="$cache/models/potion-code-16M-v2"
request="$2"
parts="$3"
missing() {
  [ -x "$program" ] || echo program
  [ -f "$model/model.safetensors" ] || echo model.safetensors
  [ -f "$model/tokenizer.json" ] || echo tokenizer.json
}
sum() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d " " -f 1
}
put() {
  case "$1" in program) target="$program" ;; *) target="$model/$1" ;; esac
  mkdir -p "${target%/*}" || exit 98
  cat "$parts/$1-$2".* | gzip -dc >"$target.$$" || { rm -f "$target.$$"; exit 98; }
  if [ "$(sum "$target.$$")" != "$2" ]; then
    rm -f "$target.$$"
    echo "checksum mismatch for $1" >&2
    exit 98
  fi
  if [ "$1" = program ]; then chmod 755 "$target.$$" || exit 98; fi
  mv -f "$target.$$" "$target" || exit 98
}
if [ -n "$parts" ]; then
  shift 3
  while [ $# -gt 1 ]; do
    put "$1" "$2"
    shift 2
  done
  rm -f "$parts"/*
  rmdir "$parts"
fi
absent=$(missing)
if [ -n "$absent" ]; then
  echo $(uname -s) $(uname -m) $absent
  exit 97
fi
SEMBLE_CACHE_LOCATION="$cache" exec "$program" search "$request"
"#;

/// One file the install step rebuilds from its delivered parts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Installed {
    pub name: String,
    pub sha256: String,
}

pub(super) fn request(query: &str, root: &str, top_k: u64) -> String {
    STANDARD.encode(json!({"query": query, "root": root, "top_k": top_k}).to_string())
}

pub(super) fn search(version: &str, request: &str) -> Result<String, String> {
    quoted(&[version, request, ""])
}

pub(super) fn install_and_search(
    version: &str,
    request: &str,
    parts: &str,
    files: &[Installed],
) -> Result<String, String> {
    let mut arguments = vec![version, request, parts];
    for file in files {
        arguments.extend([file.name.as_str(), file.sha256.as_str()]);
    }
    quoted(&arguments)
}

fn quoted(arguments: &[&str]) -> Result<String, String> {
    let mut command = format!("sh -c '{SCRIPT}' byok-semantic");
    for argument in arguments {
        if argument.contains(['\'', '\\', '\n', '\r', '\0']) {
            return Err(format!(
                "SemanticSearch cannot pass this value to a shell safely: {argument:?}"
            ));
        }
        command.push_str(&format!(" '{argument}'"));
    }
    Ok(command)
}

/// What the machine lacks, and which build of the program it needs.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Missing {
    pub platform: &'static str,
    pub files: Vec<String>,
}

pub(super) fn parse_missing(stdout: &str) -> Result<Missing, String> {
    let mut words = stdout.split_whitespace();
    let system = words.next().unwrap_or_default();
    let machine = words.next().unwrap_or_default();
    let platform = match (system, machine) {
        ("Darwin", "arm64") => "darwin-arm64",
        ("Darwin", "x86_64") => "darwin-x64",
        ("Linux", "x86_64") => "linux-x64",
        ("Linux", "aarch64" | "arm64") => "linux-arm64",
        _ => {
            return Err(format!(
                "SemanticSearch has no program for this machine: {system} {machine}"
            ))
        }
    };
    let files = words.map(str::to_string).collect::<Vec<_>>();
    if files.is_empty() {
        return Err("SemanticSearch reported missing files without naming one".into());
    }
    Ok(Missing { platform, files })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_survives_single_quotes_in_every_login_shell() {
        assert!(!SCRIPT.contains(['\'', '\\']));
    }

    #[test]
    fn a_query_never_reaches_the_shell_as_text() {
        let hostile = "'; touch /tmp/owned; echo $(id) `id` \\";
        let command = search("1.2.0", &request(hostile, "/repo", 5)).unwrap();
        assert!(!command.contains("touch"));
        assert!(!command.contains("$(id)"));
    }

    #[test]
    fn a_path_that_could_leave_its_quotes_is_refused() {
        let error = install_and_search("1.2.0", "e30=", "/home/o'brien", &[]).unwrap_err();
        assert!(error.contains("o'brien"), "{error}");
    }

    #[test]
    fn the_missing_report_selects_the_build_for_that_machine() {
        assert_eq!(
            parse_missing("Linux aarch64 program tokenizer.json\n").unwrap(),
            Missing {
                platform: "linux-arm64",
                files: vec!["program".into(), "tokenizer.json".into()],
            }
        );
        let error = parse_missing("FreeBSD amd64 program").unwrap_err();
        assert!(error.contains("FreeBSD amd64"), "{error}");
        assert!(parse_missing("Linux x86_64").is_err());
    }

    #[cfg(unix)]
    mod on_a_real_shell {
        use std::{
            fs,
            io::Write,
            path::Path,
            process::{Command, Output},
        };

        use flate2::{write::GzEncoder, Compression};
        use sha2::{Digest, Sha256};

        use super::super::*;

        // Stands in for byok-semantic: prints the request it was given.
        const FAKE: &str = "#!/bin/sh\necho \"$1 $2 $SEMBLE_CACHE_LOCATION\"\n";

        fn run(shell: &str, home: &Path, command: &str) -> Output {
            Command::new(shell)
                .arg("-c")
                .arg(command)
                .env("HOME", home)
                .output()
                .unwrap()
        }

        fn deliver(parts: &Path, name: &str, body: &[u8], chunk: usize) -> Installed {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
            encoder.write_all(body).unwrap();
            let sha256 = hex::encode(Sha256::digest(body));
            fs::create_dir_all(parts).unwrap();
            for (index, bytes) in encoder.finish().unwrap().chunks(chunk).enumerate() {
                fs::write(parts.join(format!("{name}-{sha256}.{index:03}")), bytes).unwrap();
            }
            Installed {
                name: name.into(),
                sha256,
            }
        }

        fn deliver_everything(parts: &Path) -> Vec<Installed> {
            vec![
                deliver(parts, "program", FAKE.as_bytes(), 7),
                deliver(parts, "model.safetensors", b"weights", 5),
                deliver(parts, "tokenizer.json", b"{}", 5),
            ]
        }

        #[test]
        fn a_machine_without_the_program_says_what_it_lacks() {
            let home = tempfile::tempdir().unwrap();
            let output = run("sh", home.path(), &search("1.2.0", "e30=").unwrap());
            assert_eq!(output.status.code(), Some(MISSING as i32));
            let report = String::from_utf8(output.stdout).unwrap();
            assert!(
                report
                    .trim()
                    .ends_with("program model.safetensors tokenizer.json"),
                "{report}"
            );
        }

        #[test]
        fn delivered_parts_become_a_working_search_and_leave_nothing_behind() {
            let home = tempfile::tempdir().unwrap();
            // A space in the path must not split the argument.
            let parts = home.path().join("my project/assets/byok-semantic");
            let files = deliver_everything(&parts);
            let command =
                install_and_search("1.2.0", "e30=", parts.to_str().unwrap(), &files).unwrap();
            let output = run("sh", home.path(), &command);
            assert!(output.status.success(), "{output:?}");
            let cache = home.path().join(".cursor-byok-v3/cache/semble");
            let expected = format!("search e30= {}\n", cache.display());
            assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
            assert!(!parts.exists());

            let again = run("sh", home.path(), &search("1.2.0", "e30=").unwrap());
            assert!(again.status.success(), "{again:?}");
        }

        #[test]
        fn a_damaged_delivery_installs_nothing() {
            let home = tempfile::tempdir().unwrap();
            let parts = home.path().join("parts");
            let mut files = deliver_everything(&parts);
            let wrong = "0".repeat(64);
            for entry in fs::read_dir(&parts).unwrap() {
                let path = entry.unwrap().path();
                let name = path.file_name().unwrap().to_str().unwrap().to_string();
                if name.starts_with("program-") {
                    let renamed = name.replace(&files[0].sha256, &wrong);
                    fs::rename(&path, parts.join(renamed)).unwrap();
                }
            }
            files[0].sha256 = wrong;
            let command =
                install_and_search("1.2.0", "e30=", parts.to_str().unwrap(), &files).unwrap();
            let output = run("sh", home.path(), &command);
            assert_eq!(output.status.code(), Some(98));
            assert!(String::from_utf8_lossy(&output.stderr).contains("checksum mismatch"));
            let installed = home.path().join(".cursor-byok-v3/semantic/1.2.0");
            assert_eq!(fs::read_dir(installed).unwrap().count(), 0);
        }

        #[test]
        fn zsh_and_bash_run_the_command_the_same_way() {
            for shell in ["bash", "zsh"] {
                if Command::new(shell).arg("-c").arg(":").output().is_err() {
                    continue;
                }
                let home = tempfile::tempdir().unwrap();
                let output = run(shell, home.path(), &search("1.2.0", "e30=").unwrap());
                assert_eq!(output.status.code(), Some(MISSING as i32), "{shell}");
            }
        }
    }
}
