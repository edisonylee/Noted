//! A small transparent companion window. Pointer tracking exists only while
//! detached; no event tap, accessibility permission, or global input recording.
use serde::Serialize;
use std::sync::{Condvar, Mutex};
use tauri::{Emitter, Listener, Manager};

pub const LABEL: &str = "companion";
pub const WIDTH: f64 = 168.0;
pub const HEIGHT: f64 = 184.0;
pub const PET_TOP: f64 = 44.0;

#[derive(Clone, Copy, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub supported: bool,
    pub detached: bool,
    pub dragging: bool,
    pub over_app: bool,
    pub direction: i8,
}
#[derive(Clone)]
struct Session {
    generation: u64,
    can_return: bool,
    status: Status,
    size: f64,
    grab_x: f64,
    grab_y: f64,
}
pub struct Companion(Mutex<Session>, Condvar);
impl Default for Companion {
    fn default() -> Self {
        Self(
            Mutex::new(Session {
                status: Status {
                    supported: cfg!(target_os = "macos"),
                    ..Status::default()
                },
                generation: 0,
                can_return: false,
                size: 84.0,
                grab_x: 42.0,
                grab_y: 42.0,
            }),
            Condvar::new(),
        )
    }
}
pub fn status(app: &tauri::AppHandle) -> Status {
    app.state::<Companion>().0.lock().unwrap().status
}
fn publish(app: &tauri::AppHandle) {
    let _ = app.emit("companion-desktop-state", status(app));
}
pub fn check_window(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() == "main" || window.label() == LABEL {
        Ok(())
    } else {
        Err("Companion controls are only available in Noted.".into())
    }
}
fn ensure_window(app: &tauri::AppHandle) -> Result<tauri::WebviewWindow, String> {
    if let Some(window) = app.get_webview_window(LABEL) {
        return Ok(window);
    }
    let window = tauri::WebviewWindowBuilder::new(
        app,
        LABEL,
        tauri::WebviewUrl::App("index.html?window=companion".into()),
    )
    .title("Noted companion")
    .inner_size(WIDTH, HEIGHT)
    .transparent(true)
    .decorations(false)
    .shadow(false)
    .resizable(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(false)
    .visible(false)
    .accept_first_mouse(true)
    .disable_drag_drop_handler()
    .build()
    .map_err(|e| e.to_string())?;
    let handle = app.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = return_home(&handle, false);
        }
    });
    Ok(window)
}

// macOS window positions use each window's backing scale, while the cursor
// uses the primary display's scale. Normalize both to desktop points first.
fn cursor(app: &tauri::AppHandle) -> Result<(f64, f64), String> {
    let scale = app
        .primary_monitor()
        .map_err(|e| e.to_string())?
        .map(|monitor| monitor.scale_factor())
        .unwrap_or(1.0);
    let p = app.cursor_position().map_err(|e| e.to_string())?;
    Ok((p.x / scale, p.y / scale))
}
fn main_drop(
    app: &tauri::AppHandle,
    x: f64,
    y: f64,
    grab_x: f64,
    grab_y: f64,
    size: f64,
) -> Option<serde_json::Value> {
    let main = app.get_webview_window("main")?;
    if !main.is_visible().ok()? || main.is_minimized().ok()? {
        return None;
    }
    let scale = main.scale_factor().ok()?;
    let origin = main.inner_position().ok()?.to_logical::<f64>(scale);
    let bounds = main.inner_size().ok()?.to_logical::<f64>(scale);
    landing_position(
        (x, y),
        (origin.x, origin.y),
        (bounds.width, bounds.height),
        (grab_x, grab_y),
        size,
    )
}
fn landing_position(
    pointer: (f64, f64),
    origin: (f64, f64),
    bounds: (f64, f64),
    grab: (f64, f64),
    size: f64,
) -> Option<serde_json::Value> {
    let (local_x, local_y) = (pointer.0 - origin.0, pointer.1 - origin.1);
    if local_x < 8.0 || local_y < 28.0 || local_x > bounds.0 - 8.0 || local_y > bounds.1 - 8.0 {
        return None;
    }
    Some(serde_json::json!({
        "x": ((local_x - grab.0) / (bounds.0 - size).max(1.0)).clamp(0.0, 1.0),
        "y": ((local_y - grab.1) / (bounds.1 - size).max(1.0)).clamp(0.0, 1.0)
    }))
}
pub fn return_home(app: &tauri::AppHandle, show: bool) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(LABEL) {
        window.hide().map_err(|e| e.to_string())?;
    }
    {
        let state = app.state::<Companion>();
        let mut session = state.0.lock().unwrap();
        session.generation += 1;
        session.status.detached = false;
        session.status.dragging = false;
        session.status.over_app = false;
    }
    publish(app);
    if show {
        show_main(app)?;
    }
    Ok(())
}
fn show_main(app: &tauri::AppHandle) -> Result<(), String> {
    let main = app
        .get_webview_window("main")
        .ok_or("Noted's main window is unavailable.")?;
    main.show().map_err(|e| e.to_string())?;
    main.unminimize().map_err(|e| e.to_string())?;
    main.set_focus().map_err(|e| e.to_string())
}
pub fn open_chat(app: &tauri::AppHandle) -> Result<(), String> {
    show_main(app)?;
    app.emit_to("main", "assistant-shortcut", ())
        .map_err(|e| e.to_string())
}
pub fn begin_drag(
    app: &tauri::AppHandle,
    grab_x: f64,
    grab_y: f64,
    size: f64,
    follow_pointer: bool,
) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("Desktop companions are available on macOS.".into());
    }
    if ![64.0, 84.0, 104.0].contains(&size)
        || !grab_x.is_finite()
        || !grab_y.is_finite()
        || !(0.0..=size).contains(&grab_x)
        || !(0.0..=size).contains(&grab_y)
    {
        return Err("Invalid companion drag geometry.".into());
    }
    let window = ensure_window(app)?;
    let (x, y) = cursor(app)?;
    window
        .set_position(tauri::LogicalPosition::new(
            x - grab_x - (WIDTH - size) / 2.0,
            y - grab_y - PET_TOP,
        ))
        .map_err(|e| e.to_string())?;
    window
        .set_ignore_cursor_events(false)
        .map_err(|e| e.to_string())?;
    window.show().map_err(|e| e.to_string())?;
    {
        let state = app.state::<Companion>();
        let mut session = state.0.lock().unwrap();
        session.generation += 1;
        session.size = size;
        session.grab_x = grab_x;
        session.grab_y = grab_y;
        session.can_return = session.status.detached;
        session.status.detached = true;
        session.status.dragging = follow_pointer;
        session.status.over_app = false;
    }
    publish(app);
    app.state::<Companion>().1.notify_one();
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn spawn(app: tauri::AppHandle) {
    let size_app = app.clone();
    app.listen("companion-preferences", move |event| {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(event.payload()) {
            let size = match value.get("size").and_then(|v| v.as_str()) {
                Some("small") => 64.0,
                Some("medium") => 84.0,
                Some("large") => 104.0,
                _ => return,
            };
            let state = size_app.state::<Companion>();
            let mut session = state.0.lock().unwrap();
            if !session.status.dragging {
                session.size = size;
            }
        }
    });
    std::thread::spawn(move || {
        let mut last_x = 0.0;
        let mut ignored = false;
        let mut generation = 0;
        loop {
            let current = {
                let state = app.state::<Companion>();
                let session = state
                    .1
                    .wait_while(state.0.lock().unwrap(), |session| !session.status.detached)
                    .unwrap();
                session.status
            };
            std::thread::sleep(std::time::Duration::from_millis(if current.dragging {
                16
            } else {
                40
            }));
            if !current.detached {
                continue;
            }
            let Some(window) = app.get_webview_window(LABEL) else {
                let _ = return_home(&app, false);
                continue;
            };
            let Ok((x, y)) = cursor(&app) else {
                continue;
            };
            let state = app.state::<Companion>();
            let mut session = state.0.lock().unwrap().clone();
            // A return command may have run while the cursor query was queued.
            if !session.status.detached {
                continue;
            }
            if generation != session.generation {
                generation = session.generation;
                ignored = false;
            }
            let before = session.status;
            let left = (WIDTH - session.size) / 2.0;
            if session.status.dragging {
                let drop = main_drop(&app, x, y, session.grab_x, session.grab_y, session.size);
                if drop.is_none() {
                    session.can_return = true;
                }
                session.status.over_app = session.can_return && drop.is_some();
                if (x - last_x).abs() > 1.0 {
                    session.status.direction = if x < last_x { -1 } else { 1 };
                } else {
                    session.status.direction = 0;
                }
                last_x = x;
                let _ = window.set_position(tauri::LogicalPosition::new(
                    x - session.grab_x - left,
                    y - session.grab_y - PET_TOP,
                ));
                let released = !cidre::cg::EventSrcState::CombinedSession
                    .button_state(cidre::cg::MouseButton::Left);
                if released {
                    session.status.dragging = false;
                    session.status.over_app = false;
                    if let Some(position) = drop.filter(|_| session.can_return) {
                        let active = state.0.lock().unwrap().generation == session.generation;
                        if active {
                            let _ = app.emit_to("main", "companion-returned", position);
                            let _ = return_home(&app, false);
                        }
                        continue;
                    }
                    keep_on_screen(&window);
                }
            } else if let (Ok(origin), Ok(scale)) = (window.inner_position(), window.scale_factor())
            {
                let origin = origin.to_logical::<f64>(scale);
                let (px, py) = (x - origin.x, y - origin.y);
                let pet = px >= left
                    && px <= left + session.size
                    && py >= PET_TOP
                    && py <= PET_TOP + session.size;
                let controls = px >= 16.0 && px <= WIDTH - 16.0 && py >= 150.0 && py <= HEIGHT;
                let next = !(pet || controls);
                if ignored != next {
                    let _ = window.set_ignore_cursor_events(next);
                    ignored = next;
                }
            }
            let changed = session.status != before;
            let mut live = state.0.lock().unwrap();
            if live.generation == session.generation {
                live.status = session.status;
                live.can_return = session.can_return;
                drop(live);
                if changed {
                    publish(&app);
                }
            }
        }
    });
}
#[cfg(target_os = "macos")]
fn keep_on_screen(window: &tauri::WebviewWindow) {
    if let (Ok(Some(monitor)), Ok(origin), Ok(scale)) = (
        window.current_monitor(),
        window.outer_position(),
        window.scale_factor(),
    ) {
        let origin = origin.to_logical::<f64>(scale);
        let monitor_origin = monitor.position().to_logical::<f64>(monitor.scale_factor());
        let monitor_size = monitor.size().to_logical::<f64>(monitor.scale_factor());
        let left = monitor_origin.x;
        let top = monitor_origin.y + 28.0;
        let _ = window.set_position(tauri::LogicalPosition::new(
            origin
                .x
                .clamp(left, (left + monitor_size.width - WIDTH).max(left)),
            origin.y.clamp(
                top,
                (monitor_origin.y + monitor_size.height - HEIGHT).max(top),
            ),
        ));
    }
}
#[cfg(not(target_os = "macos"))]
pub fn spawn(_app: tauri::AppHandle) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landing_preserves_the_grab_point_on_negative_origin_displays() {
        let value = landing_position(
            (-1158.0, 342.0),
            (-1600.0, 100.0),
            (1000.0, 700.0),
            (42.0, 42.0),
            84.0,
        )
        .unwrap();
        assert!((value["x"].as_f64().unwrap() - 400.0 / 916.0).abs() < 0.00001);
        assert!((value["y"].as_f64().unwrap() - 200.0 / 616.0).abs() < 0.00001);
    }

    #[test]
    fn titlebar_and_outside_drops_do_not_attach() {
        for pointer in [
            (500.0, 20.0),
            (-10.0, 400.0),
            (1001.0, 400.0),
            (500.0, 710.0),
        ] {
            assert!(
                landing_position(pointer, (0.0, 0.0), (1000.0, 700.0), (42.0, 42.0), 84.0)
                    .is_none()
            );
        }
    }

    #[test]
    fn edge_landings_are_reachable_even_when_grabbed_off_center() {
        let value = landing_position(
            (8.0, 30.0),
            (0.0, 0.0),
            (1000.0, 700.0),
            (100.0, 100.0),
            104.0,
        )
        .unwrap();
        assert_eq!(value, serde_json::json!({"x": 0.0, "y": 0.0}));
    }
}
