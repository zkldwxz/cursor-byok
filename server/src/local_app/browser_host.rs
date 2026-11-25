//! Tells the Chrome extension where BYOK listens, over Chrome's native messaging.
//!
//! Chrome starts this program for the one extension its manifest names, so no
//! web page and no other extension can ask, and nobody has to pair anything.

use std::{
    ffi::OsStr,
    fs,
    io::{self, Read, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    process::ExitCode,
};

use serde_json::{json, Value};

use crate::{config::managed_data_dir, cursor::tools::browser_bridge::BRIDGE, Result};

const NAME: &str = "dev.cursor_byok.browser";
/// The id follows from the `key` in the extension's manifest.
const EXTENSION: &str = "chrome-extension://hpeognnpkjjcacmfpofnegampmnmhmno/";
const PLACE: &str = "browser.json";

/// Answers Chrome, if that is what this program was started for.
pub fn run_if_requested() -> Option<ExitCode> {
    if !std::env::args_os().any(|arg| arg == OsStr::new(EXTENSION)) {
        return None;
    }
    let answered = managed_data_dir().and_then(|directory| {
        answer(&directory, &mut io::stdin().lock(), &mut io::stdout())?;
        Ok(())
    });
    Some(match answered {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("cannot answer the browser extension: {error}");
            ExitCode::FAILURE
        }
    })
}

/// Lets the extension of this user's browsers find BYOK at this address.
pub fn open(address: SocketAddr) -> Result<()> {
    let directory = managed_data_dir()?;
    announce(&directory, address, BRIDGE.token())?;
    register(&directory, &manifest(&program()?))
}

pub fn close() -> Result<()> {
    Ok(fs::remove_file(managed_data_dir()?.join(PLACE))?)
}

/// There is one thing to ask, so what the message says does not matter.
fn answer(directory: &Path, input: &mut impl Read, output: &mut impl Write) -> io::Result<()> {
    let mut length = [0; 4];
    input.read_exact(&mut length)?;
    let mut message = input.take(u32::from_ne_bytes(length).into());
    io::copy(&mut message, &mut io::sink())?;
    let reply = match fs::read(directory.join(PLACE)) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => b"{\"closed\":true}".to_vec(),
        read => read?,
    };
    output.write_all(&(reply.len() as u32).to_ne_bytes())?;
    output.write_all(&reply)?;
    output.flush()
}

fn announce(directory: &Path, address: SocketAddr, token: &str) -> Result<()> {
    let place = json!({"origin": format!("http://{address}"), "token": token});
    let written = directory.join(format!("{PLACE}.tmp"));
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options
        .open(&written)?
        .write_all(place.to_string().as_bytes())?;
    fs::rename(written, directory.join(PLACE))?;
    Ok(())
}

fn manifest(program: &Path) -> Value {
    json!({
        "name": NAME,
        "description": "Tells the Cursor DevTools Bridge extension where Cursor BYOK listens",
        "path": program,
        "type": "stdio",
        "allowed_origins": [EXTENSION],
    })
}

fn program() -> Result<PathBuf> {
    // What an AppImage runs from is gone once it quits.
    match std::env::var_os("APPIMAGE") {
        Some(image) => Ok(image.into()),
        None => Ok(std::env::current_exe()?),
    }
}

#[cfg(not(windows))]
fn register(_directory: &Path, manifest: &Value) -> Result<()> {
    const BROWSERS: &[&str] = if cfg!(target_os = "macos") {
        &[
            "Google/Chrome",
            "Chromium",
            "Microsoft Edge",
            "BraveSoftware/Brave-Browser",
            "Arc/User Data",
        ]
    } else {
        &[
            "google-chrome",
            "chromium",
            "microsoft-edge",
            "BraveSoftware/Brave-Browser",
        ]
    };
    let profiles = dirs::config_dir().ok_or_else(|| {
        crate::Error::Config("cannot resolve user configuration directory".into())
    })?;
    for browser in BROWSERS {
        register_in(&profiles.join(browser), manifest)?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn register_in(browser: &Path, manifest: &Value) -> Result<()> {
    if browser.is_dir() {
        let hosts = browser.join("NativeMessagingHosts");
        fs::create_dir_all(&hosts)?;
        fs::write(
            hosts.join(format!("{NAME}.json")),
            serde_json::to_vec_pretty(manifest)?,
        )?;
    }
    Ok(())
}

/// Edge and Brave look where Chrome does too.
#[cfg(windows)]
fn register(directory: &Path, manifest: &Value) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};

    let file = directory.join(format!("{NAME}.json"));
    fs::write(&file, serde_json::to_vec_pretty(manifest)?)?;
    let value: Vec<u16> = file.as_os_str().encode_wide().chain([0]).collect();
    for browser in ["Google\\Chrome", "Chromium"] {
        let key = format!("Software\\{browser}\\NativeMessagingHosts\\{NAME}");
        let key: Vec<u16> = key.encode_utf16().chain([0]).collect();
        // SAFETY: both strings end in NUL and outlive the call, which copies them.
        let status = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                std::ptr::null(),
                REG_SZ,
                value.as_ptr().cast(),
                std::mem::size_of_val(value.as_slice()) as u32,
            )
        };
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status as i32).into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asked(directory: &Path) -> Value {
        let mut input = 2_u32.to_ne_bytes().to_vec();
        input.extend(b"{}");
        let mut output = Vec::new();
        answer(directory, &mut input.as_slice(), &mut output).unwrap();
        let (length, reply) = output.split_at(4);
        assert_eq!(
            u32::from_ne_bytes(length.try_into().unwrap()) as usize,
            reply.len()
        );
        serde_json::from_slice(reply).unwrap()
    }

    #[test]
    fn the_extension_hears_where_byok_is_now_and_that_it_is_closed() {
        let directory = tempfile::tempdir().unwrap();
        let directory = directory.path();
        assert_eq!(asked(directory), json!({"closed": true}));
        for (port, token) in [(3010, "a"), (3011, "b")] {
            announce(directory, SocketAddr::from(([127, 0, 0, 1], port)), token).unwrap();
            let origin = format!("http://127.0.0.1:{port}");
            assert_eq!(asked(directory), json!({"origin": origin, "token": token}));
        }
    }

    #[cfg(unix)]
    #[test]
    fn nobody_else_on_the_computer_can_read_the_token() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        announce(directory.path(), "127.0.0.1:1".parse().unwrap(), "a").unwrap();
        let place = fs::metadata(directory.path().join(PLACE)).unwrap();
        assert_eq!(place.permissions().mode() & 0o777, 0o600);
    }

    #[cfg(not(windows))]
    #[test]
    fn only_browsers_there_are_get_a_host_and_only_our_extension_may_use_it() {
        let directory = tempfile::tempdir().unwrap();
        let (chrome, missing) = (
            directory.path().join("chrome"),
            directory.path().join("edge"),
        );
        fs::create_dir(&chrome).unwrap();
        let manifest = manifest(Path::new("/opt/byok"));
        register_in(&chrome, &manifest).unwrap();
        register_in(&missing, &manifest).unwrap();
        assert!(!missing.exists());
        let file = chrome.join("NativeMessagingHosts/dev.cursor_byok.browser.json");
        let written: Value = serde_json::from_slice(&fs::read(file).unwrap()).unwrap();
        assert_eq!(written["path"], "/opt/byok");
        assert_eq!(written["allowed_origins"], json!([EXTENSION]));
    }

    #[test]
    fn a_message_cut_short_is_an_error() {
        let directory = tempfile::tempdir().unwrap();
        let reply = answer(directory.path(), &mut [1, 0].as_slice(), &mut Vec::new());
        assert!(reply.is_err());
    }
}
