//! Runs the phone app and gives its page the connection to the computer.
mod connection;

use tauri::{ipc::Channel, State};

use connection::{Connection, Heard};

#[tauri::command]
async fn dial(
    connection: State<'_, Connection>,
    ticket: String,
    events: Channel<Heard>,
) -> Result<u32, String> {
    connection
        .dial(&ticket, move |heard| events.send(heard).is_ok())
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn say(connection: State<'_, Connection>, id: u32, text: String) {
    connection.say(id, text);
}

#[tauri::command]
fn direct(connection: State<'_, Connection>) -> Option<bool> {
    connection.direct()
}

#[tauri::command]
fn hang_up(connection: State<'_, Connection>, id: u32) {
    connection.hang_up(id);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(mobile)]
    let builder = builder.plugin(tauri_plugin_barcode_scanner::init());
    builder
        .manage(Connection::default())
        .invoke_handler(tauri::generate_handler![dial, say, direct, hang_up])
        .run(tauri::generate_context!())
        .expect("the app did not start");
}
