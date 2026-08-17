use crate::mobile_store::{MobileNote, MobileStore};
use serde::Serialize;
use std::fs;
use tauri::{Manager, State};

#[derive(Serialize)]
struct MobileHealth {
    platform: &'static str,
    storage: &'static str,
    sync: &'static str,
}

#[tauri::command]
fn mobile_health() -> MobileHealth {
    MobileHealth {
        platform: "ios",
        storage: "ready",
        sync: "not_enrolled",
    }
}

#[tauri::command]
fn list_mobile_notes(
    store: State<'_, MobileStore>,
    query: Option<String>,
) -> Result<Vec<MobileNote>, String> {
    store.list(query.as_deref())
}

#[tauri::command]
fn create_mobile_note(
    store: State<'_, MobileStore>,
    title: String,
    body: String,
) -> Result<MobileNote, String> {
    store.create(&title, &body)
}

#[tauri::command]
fn update_mobile_note(
    store: State<'_, MobileStore>,
    record_id: String,
    title: String,
    body: String,
) -> Result<MobileNote, String> {
    store.update(&record_id, &title, &body)
}

#[tauri::command]
fn delete_mobile_note(store: State<'_, MobileStore>, record_id: String) -> Result<(), String> {
    store.delete(&record_id)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            fs::create_dir_all(&data_dir)?;
            let store = MobileStore::open(&data_dir.join("noted-mobile.sqlite3"))
                .map_err(std::io::Error::other)?;
            app.manage(store);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            mobile_health,
            list_mobile_notes,
            create_mobile_note,
            update_mobile_note,
            delete_mobile_note
        ])
        .run(tauri::generate_context!())
        .expect("error while running Noted on iOS");
}
