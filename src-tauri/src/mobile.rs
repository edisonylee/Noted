use serde::Serialize;

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
        storage: "not_initialized",
        sync: "not_enrolled",
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![mobile_health])
        .run(tauri::generate_context!())
        .expect("error while running Noted on iOS");
}
