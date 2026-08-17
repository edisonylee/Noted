use crate::mobile_deep_link::MobileDeepLink;
use crate::mobile_store::{MobileNote, MobileNotesWorkspace, MobileStore, MobileWorkspaceNote};
use serde::Serialize;
use std::fs;
use tauri::{Manager, State};

#[derive(Serialize)]
struct MobileHealth {
    platform: &'static str,
    storage: String,
    sync: String,
}

#[tauri::command]
fn mobile_health(store: State<'_, MobileStore>) -> Result<MobileHealth, String> {
    let health = store.health()?;
    Ok(MobileHealth {
        platform: "ios",
        storage: health.storage,
        sync: health.sync,
    })
}

#[tauri::command]
fn list_mobile_notes(
    store: State<'_, MobileStore>,
    query: Option<String>,
) -> Result<Vec<MobileNote>, String> {
    store.list(query.as_deref())
}

#[tauri::command]
fn get_mobile_notes_workspace(
    store: State<'_, MobileStore>,
    query: Option<String>,
    view: Option<String>,
    folder_id: Option<String>,
) -> Result<MobileNotesWorkspace, String> {
    store.workspace(query.as_deref(), view.as_deref(), folder_id.as_deref())
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

#[tauri::command]
fn trash_mobile_note(store: State<'_, MobileStore>, record_id: String) -> Result<(), String> {
    store.delete(&record_id)
}

#[tauri::command]
fn restore_mobile_note(
    store: State<'_, MobileStore>,
    record_id: String,
) -> Result<MobileNote, String> {
    store.restore(&record_id)
}

#[tauri::command]
fn file_mobile_note(
    store: State<'_, MobileStore>,
    record_id: String,
    folder_id: String,
) -> Result<MobileWorkspaceNote, String> {
    store.file_note(&record_id, &folder_id)
}

#[tauri::command]
fn undo_mobile_note_filing(
    store: State<'_, MobileStore>,
    record_id: String,
) -> Result<MobileWorkspaceNote, String> {
    store.undo_note_filing(&record_id)
}

#[tauri::command]
fn resolve_mobile_note_conflict(
    store: State<'_, MobileStore>,
    record_id: String,
    resolution: String,
) -> Result<MobileWorkspaceNote, String> {
    store.resolve_note_conflict(&record_id, &resolution)
}

#[tauri::command]
fn resolve_mobile_deep_link(
    store: State<'_, MobileStore>,
    url: String,
) -> Result<MobileDeepLink, String> {
    let link = MobileDeepLink::parse(&url).map_err(|error| error.to_string())?;
    match &link {
        MobileDeepLink::Note {
            library_id,
            record_id,
        } => store.verify_note_link(library_id, record_id)?,
    }
    Ok(link)
}

#[tauri::command]
fn export_mobile_notes(store: State<'_, MobileStore>) -> Result<String, String> {
    store.export_notes()
}

#[tauri::command]
fn restore_mobile_notes_export(
    store: State<'_, MobileStore>,
    export_json: String,
) -> Result<usize, String> {
    store.restore_notes_export(&export_json)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_deep_link::init())
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
            get_mobile_notes_workspace,
            list_mobile_notes,
            create_mobile_note,
            update_mobile_note,
            delete_mobile_note,
            trash_mobile_note,
            restore_mobile_note,
            file_mobile_note,
            undo_mobile_note_filing,
            resolve_mobile_note_conflict,
            resolve_mobile_deep_link,
            export_mobile_notes,
            restore_mobile_notes_export
        ])
        .run(tauri::generate_context!())
        .expect("error while running Noted on iOS");
}
