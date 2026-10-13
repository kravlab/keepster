//! The macOS tray: an `NSStatusItem` in the menu bar and notifications through
//! `UNUserNotificationCenter`, with AppKit's run loop on the main thread.
//!
//! The connection runs on its own thread and hands each call to the main thread with
//! `performSelectorOnMainThread:`; everything AppKit owns stays on the main thread. Notifications need a
//! bundle identifier, which only an `.app` has (ticket 68): run from anywhere else, the tray says
//! `no_bundle` rather than ask the notification center, which would raise an exception.
#![allow(
    unsafe_code,
    reason = "defining an Objective-C class, selector-based dispatch and menu targets have no safe \
              wrapper in objc2; they are confined to this module"
)]

use std::cell::RefCell;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObject};
use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSEvent, NSEventModifierFlags, NSEventType,
    NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{NSBundle, NSError, NSObjectNSThreadPerformAdditions, NSPoint, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationRequest,
    UNUserNotificationCenter,
};
use tokio::sync::mpsc as tokio_mpsc;

use crate::menu;
use crate::model::{Icon, Notice, View};
use crate::platform::capabilities::Capabilities;
use crate::platform::handoff::{Command, Handoff};
use crate::platform::{Connection, Shell};
use crate::run::{Action, Adapter};
use crate::strings;

/// Why the notifications row is unavailable: the tray runs outside an `.app`, so it has no bundle
/// identifier.
const NO_BUNDLE: &str = "no_bundle";
/// Why the notifications row is unavailable: the person has not allowed notifications, or not yet.
const NOT_AUTHORIZED: &str = "not_authorized";

thread_local! {
    /// The menu bar item, owned by the main thread, which alone may touch it.
    static STATUS_ITEM: RefCell<Option<Retained<NSStatusItem>>> = const { RefCell::new(None) };
}

/// The macOS shell: AppKit's run loop, on the main thread.
pub struct MacOs {
    actions: tokio_mpsc::UnboundedSender<Action>,
}

impl MacOs {
    /// A shell sending what is chosen from the menu to `actions`. Nothing touches AppKit until
    /// [`Shell::run`].
    pub fn new(actions: tokio_mpsc::UnboundedSender<Action>) -> Self {
        Self { actions }
    }
}

impl Shell for MacOs {
    fn run(self: Box<Self>, connection: Connection) {
        let Some(mtm) = MainThreadMarker::new() else {
            tracing::error!("the tray was not started on the main thread, which AppKit needs");
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        // A menu bar item and nothing else: no Dock icon, no menu bar of its own.
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

        let report = Capabilities::default();
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        STATUS_ITEM.with(|slot| *slot.borrow_mut() = Some(item));
        report.set_icon(Ok(()));
        let bundled = NSBundle::mainBundle().bundleIdentifier().is_some();
        if bundled {
            ask_to_notify(&report);
        } else {
            report.set_notifications(Err(NO_BUNDLE));
        }

        let (commands_tx, commands) = mpsc::channel();
        let target = Target::new(commands, self.actions, report.clone(), bundled);
        let woken = target.clone();
        let adapter: Arc<dyn Adapter> = Arc::new(Handoff::new(report, move |command| {
            if commands_tx.send(command).is_ok() {
                wake(&woken);
            }
        }));
        // Started only now, so that its first report already says what the item and the notification
        // center can do. Once it ends its adapter is gone, and the drain it asks for stops the run loop.
        let waker = target.clone();
        let connection = std::thread::spawn(move || {
            connection(adapter);
            wake(&waker);
        });
        app.run();
        let _ = connection.join();

        if let Some(item) = STATUS_ITEM.with(|slot| slot.borrow_mut().take()) {
            NSStatusBar::systemStatusBar().removeStatusItem(&item);
        }
    }
}

/// Ask once to show notifications; the row says whether that was granted. Until the person answers, it
/// says they are not authorised, which is the truth until then.
fn ask_to_notify(report: &Capabilities) {
    report.set_notifications(Err(NOT_AUTHORIZED));
    let answered = report.clone();
    let handler = RcBlock::new(move |granted: Bool, _error: *mut NSError| {
        answered.set_notifications(if granted.as_bool() {
            Ok(())
        } else {
            Err(NOT_AUTHORIZED)
        });
    });
    UNUserNotificationCenter::currentNotificationCenter()
        .requestAuthorizationWithOptions_completionHandler(UNAuthorizationOptions::Alert, &handler);
}

/// Have the main thread drain what the connection asked for.
fn wake(target: &Target) {
    // SAFETY: `drain` is a method `Target` defines, taking no argument, which `None` matches; asking for
    // it from any thread is what `performSelectorOnMainThread:` is for.
    unsafe {
        target.performSelectorOnMainThread_withObject_waitUntilDone(sel!(drain), None, false)
    };
}

/// `Target`'s state. Every field is safe to share between threads, so a `Target` may be woken from the
/// connection's thread; what only the main thread may touch lives in [`STATUS_ITEM`].
struct Ivars {
    /// What the connection queued for the main thread.
    commands: Mutex<Receiver<Command>>,
    /// Where what is chosen from the menu goes.
    actions: tokio_mpsc::UnboundedSender<Action>,
    /// What each item of the menu last drawn does, by its tag.
    chosen: Mutex<Vec<Option<Action>>>,
    /// The notifications row, as the notification center last answered.
    report: Capabilities,
    /// Whether the tray runs from an `.app`, without which the notification center is not asked.
    bundled: bool,
}

define_class!(
    // SAFETY: `NSObject` has no subclassing requirements, and `Target` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[ivars = Ivars]
    struct Target;

    impl Target {
        /// Do what the connection queued, on the main thread; stop the run loop once the connection
        /// has ended.
        #[unsafe(method(drain))]
        fn drain(&self) {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            loop {
                let next = self
                    .ivars()
                    .commands
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .try_recv();
                match next {
                    Ok(Command::Show(view)) => self.show(mtm, &view),
                    Ok(Command::Notify(notice)) => self.notify(&notice),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        stop(mtm);
                        break;
                    }
                }
            }
        }

        /// A menu item was chosen: pass on what it does.
        #[unsafe(method(chosen:))]
        fn chosen(&self, item: &NSMenuItem) {
            let action = usize::try_from(item.tag()).ok().and_then(|tag| {
                self.ivars()
                    .chosen
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(tag)
                    .copied()
                    .flatten()
            });
            if let Some(action) = action {
                let _ = self.ivars().actions.send(action);
            }
        }
    }
);

impl Target {
    /// A target draining `commands` and passing what is chosen to `actions`.
    fn new(
        commands: Receiver<Command>,
        actions: tokio_mpsc::UnboundedSender<Action>,
        report: Capabilities,
        bundled: bool,
    ) -> Retained<Self> {
        let this = Self::alloc().set_ivars(Ivars {
            commands: Mutex::new(commands),
            actions,
            chosen: Mutex::new(Vec::new()),
            report,
            bundled,
        });
        // SAFETY: `NSObject`'s designated initialiser, on an instance whose ivars are set.
        unsafe { msg_send![super(this), init] }
    }

    /// Draw the item, its tooltip and its menu as `view` says.
    fn show(&self, mtm: MainThreadMarker, view: &View) {
        let status = NSString::from_str(&view.status);
        let items = menu::items(view);
        let menu = NSMenu::new(mtm);
        // Each item is enabled or not as the model says, not as AppKit would validate it.
        menu.setAutoenablesItems(false);
        for (tag, item) in items.iter().enumerate() {
            // SAFETY: `chosen:` is a method `Target` defines, taking the item that sends it.
            let entry = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &NSString::from_str(&item.label),
                    Some(sel!(chosen:)),
                    &NSString::new(),
                )
            };
            // SAFETY: the target is not retained by the item; `self` outlives the menu, being held by
            // the shell until the run loop has stopped.
            unsafe { entry.setTarget(Some(self)) };
            entry.setTag(isize::try_from(tag).unwrap_or(isize::MAX));
            entry.setEnabled(item.action.is_some());
            menu.addItem(&entry);
        }
        *self
            .ivars()
            .chosen
            .lock()
            .unwrap_or_else(PoisonError::into_inner) =
            items.iter().map(|item| item.action).collect();

        STATUS_ITEM.with(|slot| {
            let Some(item) = slot.borrow().clone() else {
                return;
            };
            if let Some(button) = item.button(mtm) {
                match NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str(symbol(view.icon)),
                    Some(&status),
                ) {
                    Some(image) => {
                        // Drawn in the menu bar's own colours, light or dark.
                        image.setTemplate(true);
                        button.setImage(Some(&image));
                    }
                    // A system without SF Symbols still shows the item, by name.
                    None => button.setTitle(&NSString::from_str(strings::APP_NAME)),
                }
                button.setToolTip(Some(&status));
            }
            item.setMenu(Some(&menu));
        });
    }

    /// Show `notice`, replacing the one shown under its key: the notification center replaces a
    /// request by its identifier.
    fn notify(&self, notice: &Notice) {
        if !self.ivars().bundled {
            return;
        }
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&notice.title));
        content.setBody(&NSString::from_str(&notice.body));
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(&notice.key.identifier()),
            &content,
            None,
        );
        let report = self.ivars().report.clone();
        let title = notice.title.clone();
        let handler = RcBlock::new(move |error: *mut NSError| {
            if error.is_null() {
                report.set_notifications(Ok(()));
            } else {
                tracing::warn!(%title, "the notification center refused the notification");
                report.set_notifications(Err(NOT_AUTHORIZED));
            }
        });
        UNUserNotificationCenter::currentNotificationCenter()
            .addNotificationRequest_withCompletionHandler(&request, Some(&handler));
    }
}

/// Stop the run loop: `stop:` takes effect once an event has been handled, so one is posted.
fn stop(mtm: MainThreadMarker) {
    let app = NSApplication::sharedApplication(mtm);
    app.stop(None);
    if let Some(event) = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
        NSEventType::ApplicationDefined,
        NSPoint::new(0.0, 0.0),
        NSEventModifierFlags::empty(),
        0.0,
        0,
        None,
        0,
        0,
        0,
    ) {
        app.postEvent_atStart(&event, true);
    }
}

/// The SF Symbol drawn for `icon`.
fn symbol(icon: Icon) -> &'static str {
    match icon {
        Icon::Covered => "checkmark.shield",
        Icon::Attention => "exclamationmark.triangle",
        Icon::Stopped => "stop.circle",
    }
}
