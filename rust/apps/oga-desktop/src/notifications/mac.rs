//! macOS delivery. The system notification centre carries the banner, and
//! hands back the task it was about when the user clicks it.

#![allow(deprecated)]

use objc2::{
    AllocAnyThread, DefinedClass, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, ProtocolObject},
};
use objc2_foundation::{
    NSBundle, NSDictionary, NSObject, NSObjectProtocol, NSString, NSUserNotification,
    NSUserNotificationCenter, NSUserNotificationCenterDelegate,
};
use tauri::{AppHandle, Runtime};

use super::{Message, open_task};

/// Where the notification carries the task it speaks for.
const TASK_KEY: &str = "ogaTaskId";

type Route = Box<dyn Fn(&str)>;

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "OgaNotificationDelegate"]
    #[ivars = Route]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl NSUserNotificationCenterDelegate for Delegate {
        #[unsafe(method(userNotificationCenter:didActivateNotification:))]
        fn did_activate(
            &self,
            _center: &NSUserNotificationCenter,
            notification: &NSUserNotification,
        ) {
            if let Some(task_id) = task_id(notification) {
                (self.ivars())(&task_id);
            }
        }

        /// Oga stays quiet about the task already on screen, so anything that
        /// reaches the centre is worth showing even with the app in front.
        #[unsafe(method(userNotificationCenter:shouldPresentNotification:))]
        fn should_present(
            &self,
            _center: &NSUserNotificationCenter,
            _notification: &NSUserNotification,
        ) -> bool {
            true
        }
    }
);

impl Delegate {
    fn new(route: Route) -> Retained<Self> {
        let this = Self::alloc().set_ivars(route);
        unsafe { msg_send![super(this), init] }
    }
}

/// Answers a click with the task the user asked to see.
pub fn route_clicks<R: Runtime>(app: AppHandle<R>) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let delegate = Delegate::new(Box::new(move |task_id: &str| open_task(&handle, task_id)));
        let center = NSUserNotificationCenter::defaultUserNotificationCenter();
        unsafe { center.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
        // The centre holds its delegate weakly, and the click route has to
        // outlive this call for as long as the app runs.
        std::mem::forget(delegate);
    });
}

pub fn show<R: Runtime>(app: &AppHandle<R>, message: Message) {
    let _ = app.run_on_main_thread(move || {
        if !packaged() {
            return;
        }
        let notification = NSUserNotification::new();
        notification.setTitle(Some(&NSString::from_str(&message.title)));
        notification.setInformativeText(Some(&NSString::from_str(&message.body)));
        if let Some(task_id) = message.task_id.as_deref() {
            let key = NSString::from_str(TASK_KEY);
            let value = NSString::from_str(task_id);
            let info: Retained<NSDictionary<NSString, AnyObject>> =
                NSDictionary::from_slices(&[&*key], &[value.as_ref() as &AnyObject]);
            unsafe { notification.setUserInfo(Some(&info)) };
        }
        let center = NSUserNotificationCenter::defaultUserNotificationCenter();
        center.deliverNotification(&notification);
    });
}

fn task_id(notification: &NSUserNotification) -> Option<String> {
    let info = notification.userInfo()?;
    let value = info.objectForKey(&*NSString::from_str(TASK_KEY))?;
    value
        .downcast::<NSString>()
        .ok()
        .map(|task_id| task_id.to_string())
}

/// A development build runs straight from its binary, with no app identity for
/// the notification centre to put on a banner.
fn packaged() -> bool {
    NSBundle::mainBundle().bundleIdentifier().is_some()
}
