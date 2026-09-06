use serde::{Deserialize, Serialize};
use std::sync::Mutex;
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Target {
    pub server: String,
    pub org: String,
    pub user: String,
    pub message: String,
}
static PENDING: Mutex<Option<Target>> = Mutex::new(None);
pub fn take_target() -> Option<Target> {
    PENDING.lock().unwrap().take()
}
// Team alerts use UserNotifications with explicit foreground presentation.
#[cfg(target_os = "macos")]
mod mac {
    use super::{Target, PENDING};
    use std::sync::OnceLock;
    use tauri::{Emitter, Manager};
    static APP: OnceLock<tauri::AppHandle> = OnceLock::new();
    use block2::RcBlock;
    use objc2::{define_class, msg_send, rc::Retained, runtime::ProtocolObject, ClassType};
    use objc2_foundation::{NSBundle, NSError, NSObject, NSObjectProtocol, NSString};
    use objc2_user_notifications::*;
    use std::sync::Mutex;

    define_class!(
        #[unsafe(super(NSObject))]
        #[name = "NotedTeamNotificationDelegate"]
        struct Delegate;
        unsafe impl NSObjectProtocol for Delegate {}
        unsafe impl UNUserNotificationCenterDelegate for Delegate {
            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            fn opened(
                &self,
                _center: &UNUserNotificationCenter,
                response: &UNNotificationResponse,
                completion: &block2::DynBlock<dyn Fn()>,
            ) {
                // Dismissals and custom actions must not navigate.
                if response.actionIdentifier().to_string()
                    == unsafe { UNNotificationDefaultActionIdentifier }.to_string()
                {
                    let identifier = response.notification().request().identifier().to_string();
                    if let Some(encoded) = identifier.strip_prefix("noted-route:") {
                        if let Ok((target, _nonce)) =
                            serde_json::from_str::<(Target, String)>(encoded)
                        {
                            *PENDING.lock().unwrap() = Some(target);
                            if let Some(app) = APP.get() {
                                if let Some(window) = app.get_webview_window("main") {
                                    let _ = window.show();
                                    let _ = window.unminimize();
                                    let _ = window.set_focus();
                                }
                                let _ = app.emit("team-notification-open", ());
                            }
                        }
                    }
                }
                completion.call(());
            }

            #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
            fn present(
                &self,
                _center: &UNUserNotificationCenter,
                _notification: &UNNotification,
                completion: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
            ) {
                completion.call((UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List
                    | UNNotificationPresentationOptions::Sound,));
            }
        }
    );
    thread_local! {
        // The notification center's delegate is weak; retain it for app lifetime.
        static DELEGATE: Retained<Delegate> = unsafe { msg_send![Delegate::class(), new] };
    }
    pub fn init(app: tauri::AppHandle) {
        if NSBundle::mainBundle().bundleIdentifier().is_none() {
            return;
        }
        let _ = APP.set(app);
        DELEGATE.with(|delegate| {
            UNUserNotificationCenter::currentNotificationCenter()
                .setDelegate(Some(ProtocolObject::from_ref(&**delegate)));
        });
    }
    pub async fn send(title: String, body: String, target: Option<Target>) -> Result<(), String> {
        if NSBundle::mainBundle().bundleIdentifier().is_none() {
            return Err(
                "Native notification testing requires the installed Noted app bundle.".into(),
            );
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let tx = Mutex::new(Some(tx));
            let callback =
                RcBlock::new(move |granted: objc2::runtime::Bool, error: *mut NSError| {
                    let result = if !error.is_null() {
                        Err(unsafe { (&*error).localizedDescription().to_string() })
                    } else if !granted.as_bool() {
                        Err(
                            "Notifications are not authorized for Noted in macOS System Settings."
                                .into(),
                        )
                    } else {
                        Ok(())
                    };
                    if let Some(tx) = tx.lock().unwrap().take() {
                        let _ = tx.send(result);
                    }
                });
            UNUserNotificationCenter::currentNotificationCenter()
                .requestAuthorizationWithOptions_completionHandler(
                    UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
                    &callback,
                );
        }
        tokio::time::timeout(std::time::Duration::from_secs(60), rx)
            .await
            .map_err(|_| "Notification authorization timed out")?
            .map_err(|_| "Notification authorization interrupted")??;
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let tx = Mutex::new(Some(tx));
            let callback =
                RcBlock::new(move |settings: std::ptr::NonNull<UNNotificationSettings>| {
                    let sound = unsafe { settings.as_ref().soundSetting() };
                    if let Some(tx) = tx.lock().unwrap().take() {
                        let _ = tx.send(sound == UNNotificationSetting::Enabled);
                    }
                });
            UNUserNotificationCenter::currentNotificationCenter()
                .getNotificationSettingsWithCompletionHandler(&callback);
        }
        let sound_enabled = tokio::time::timeout(std::time::Duration::from_secs(5), rx)
            .await
            .map_err(|_| "Reading notification sound settings timed out")?
            .map_err(|_| "Reading notification sound settings interrupted")?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let content = UNMutableNotificationContent::new();
            content.setTitle(&NSString::from_str(&title));
            content.setBody(&NSString::from_str(&body));
            content.setSound(Some(&UNNotificationSound::defaultSound()));
            let nonce = rand::random::<u128>().to_string();
            let identifier = NSString::from_str(&match target {
                Some(target) => format!(
                    "noted-route:{}",
                    serde_json::to_string(&(target, nonce)).map_err(|e| e.to_string())?
                ),
                None => format!("noted-team-{nonce}"),
            });
            let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
                &identifier,
                &content,
                None,
            );
            let tx = Mutex::new(Some(tx));
            let callback = RcBlock::new(move |error: *mut NSError| {
                let result = if error.is_null() {
                    Ok(())
                } else {
                    Err(unsafe { (&*error).localizedDescription().to_string() })
                };
                if let Some(tx) = tx.lock().unwrap().take() {
                    let _ = tx.send(result);
                }
            });
            UNUserNotificationCenter::currentNotificationCenter()
                .addNotificationRequest_withCompletionHandler(&request, Some(&callback));
        }
        tokio::time::timeout(std::time::Duration::from_secs(5), rx)
            .await
            .map_err(|_| "Notification submission timed out")?
            .map_err(|_| "Notification submission interrupted")??;
        if !sound_enabled {
            return Err(
                "Banner sent, but macOS reports notification sounds disabled for this Noted app."
                    .into(),
            );
        }
        Ok(())
    }
}
#[cfg(target_os = "macos")]
pub use mac::{init, send};
#[cfg(not(target_os = "macos"))]
pub fn init(_app: tauri::AppHandle) {}
#[cfg(not(target_os = "macos"))]
pub async fn send(_title: String, _body: String, _target: Option<Target>) -> Result<(), String> {
    Err("Team desktop notifications currently require macOS".into())
}
