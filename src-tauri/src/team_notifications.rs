// Team alerts use UserNotifications with explicit foreground presentation.
#[cfg(target_os = "macos")]
mod mac {
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
    pub fn init() {
        if NSBundle::mainBundle().bundleIdentifier().is_none() {
            return;
        }
        DELEGATE.with(|delegate| {
            UNUserNotificationCenter::currentNotificationCenter()
                .setDelegate(Some(ProtocolObject::from_ref(&**delegate)));
        });
    }
    pub async fn send(title: String, body: String) -> Result<(), String> {
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
            let identifier = NSString::from_str(&format!("noted-team-{nonce}"));
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
pub fn init() {}
#[cfg(not(target_os = "macos"))]
pub async fn send(_title: String, _body: String) -> Result<(), String> {
    Err("Team desktop notifications currently require macOS".into())
}
