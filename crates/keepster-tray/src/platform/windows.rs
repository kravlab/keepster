//! The Windows tray: a notification-area icon owned by a hidden window, with the window's message loop on
//! the main thread, and toasts through WinRT.
//!
//! The connection runs on its own thread and hands each call to the main thread by posting the window a
//! message; the icon, its menu and the window are only ever touched from the main thread. A toast from an
//! unpackaged program is shown only while a Start-menu shortcut carries its AppUserModelID, which
//! `keepster install` creates (ticket 68): without it the tray says `no_app_id` rather than show toasts
//! nobody sees.
#![allow(
    unsafe_code,
    reason = "the Win32 calls below have no safe wrapper in std; they are confined to this module"
)]

use std::cell::RefCell;
use std::ptr;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use keepster_service::autostart::tray::{APP_ID, shortcut_locations};
use keepster_service::to_wide;
use tokio::sync::mpsc as tokio_mpsc;
use windows::Data::Xml::Dom::XmlDocument;
use windows::UI::Notifications::{
    NotificationSetting, ToastNotification, ToastNotificationManager,
};
use windows::core::HSTRING;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, IDI_APPLICATION, IDI_ERROR, IDI_SHIELD,
    IDI_WARNING, LoadIconW, MF_GRAYED, MF_STRING, MSG, PostMessageW, PostQuitMessage,
    RegisterClassW, RegisterWindowMessageW, SetForegroundWindow, TPM_NONOTIFY, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, WM_APP, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP,
    WNDCLASSW, WS_OVERLAPPED,
};

use crate::menu;
use crate::model::{Icon, Notice, View};
use crate::platform::capabilities::Capabilities;
use crate::platform::handoff::{Command, Handoff};
use crate::platform::{Connection, Shell};
use crate::run::{Action, Adapter};
use crate::strings;

/// The message the icon sends its window when it is clicked.
const WM_ICON: u32 = WM_APP + 1;
/// The message the connection posts when it has queued something, or ended.
const WM_DRAIN: u32 = WM_APP + 2;
/// The one icon the window owns.
const ICON_ID: u32 = 1;
/// The reason the icon row is unavailable when the notification area refuses the icon.
const NO_TASKBAR: &str = "no_taskbar";
/// Why the notifications row is unavailable: no Start-menu shortcut carries the AppUserModelID.
const NO_APP_ID: &str = "no_app_id";
/// Why the notifications row is unavailable: Windows has notifications turned off for the app, for the
/// user or by policy.
const DISABLED: &str = "disabled";
/// Why the notifications row is unavailable: showing the last toast failed.
const TOAST_FAILED: &str = "toast_failed";

thread_local! {
    /// What the window procedure works with, owned by the main thread, which alone runs it.
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// The main thread's side.
struct State {
    /// What the connection queued for the main thread.
    commands: Receiver<Command>,
    /// Where what is chosen from the menu goes.
    actions: tokio_mpsc::UnboundedSender<Action>,
    /// The two rows, as the icon and the toasts last fared.
    report: Capabilities,
    /// What the icon shows now, which the menu is built from and a restarted Explorer is shown again.
    view: Option<View>,
    /// The message Explorer broadcasts when it starts again, after which the icon must be added anew.
    taskbar_created: u32,
}

/// The Windows shell: a hidden window's message loop, on the main thread.
pub struct Windows {
    actions: tokio_mpsc::UnboundedSender<Action>,
}

impl Windows {
    /// A shell sending what is chosen from the menu to `actions`. No window exists until
    /// [`Shell::run`].
    pub fn new(actions: tokio_mpsc::UnboundedSender<Action>) -> Self {
        Self { actions }
    }
}

impl Shell for Windows {
    fn run(self: Box<Self>, connection: Connection) {
        let Some(window) = create_window() else {
            tracing::error!(
                error = %std::io::Error::last_os_error(),
                "the tray's window could not be created"
            );
            return;
        };
        let report = Capabilities::default();
        report.set_icon(if notify_icon(NIM_ADD, window, None) {
            Ok(())
        } else {
            Err(NO_TASKBAR)
        });
        report.set_notifications(if shortcut_installed() {
            Ok(())
        } else {
            Err(NO_APP_ID)
        });

        let (commands_tx, commands) = mpsc::channel();
        // SAFETY: the string is NUL-terminated and outlives the call.
        let taskbar_created = unsafe { RegisterWindowMessageW(to_wide("TaskbarCreated").as_ptr()) };
        STATE.with(|state| {
            *state.borrow_mut() = Some(State {
                commands,
                actions: self.actions,
                report: report.clone(),
                view: None,
                taskbar_created,
            });
        });
        // A window handle is a plain value any thread may post to; it travels as one.
        let posted_to = window as usize;
        let adapter: Arc<dyn Adapter> = Arc::new(Handoff::new(report, move |command| {
            if commands_tx.send(command).is_ok() {
                post_drain(posted_to);
            }
        }));
        // Started only now, so that its first report already says whether the icon was added. Once it
        // ends its adapter is gone, and the drain it asks for ends the message loop.
        let connection = std::thread::spawn(move || {
            connection(adapter);
            post_drain(posted_to);
        });

        let mut message = MSG::default();
        // SAFETY: `message` is a valid out-pointer; the loop ends on `WM_QUIT` (0) or an error (-1).
        while unsafe { GetMessageW(&mut message, ptr::null_mut(), 0, 0) } > 0 {
            // SAFETY: `message` was just filled in by `GetMessageW`.
            unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        let _ = connection.join();

        notify_icon(NIM_DELETE, window, None);
        STATE.with(|state| state.borrow_mut().take());
        // SAFETY: the window was created by this thread and is destroyed once.
        unsafe { DestroyWindow(window) };
    }
}

/// Ask the window to drain what the connection queued.
fn post_drain(window: usize) {
    // SAFETY: posting is allowed from any thread; to a window already destroyed it fails harmlessly.
    unsafe { PostMessageW(window as HWND, WM_DRAIN, 0, 0) };
}

/// Register the window class and create the hidden window that owns the icon.
fn create_window() -> Option<HWND> {
    let class = to_wide("KeepsterTray");
    // SAFETY: a null name asks for this executable's own module.
    let instance = unsafe { GetModuleHandleW(ptr::null()) };
    let class_definition = WNDCLASSW {
        lpfnWndProc: Some(procedure),
        hInstance: instance,
        lpszClassName: class.as_ptr(),
        ..Default::default()
    };
    // SAFETY: the definition and the class name it points to outlive the call.
    if unsafe { RegisterClassW(&class_definition) } == 0 {
        return None;
    }
    let title = to_wide(strings::APP_NAME);
    // SAFETY: the class was registered above; both strings are NUL-terminated and outlive the call. The
    // window is never shown: it exists to receive the icon's messages and Explorer's broadcasts.
    let window = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            ptr::null(),
        )
    };
    (!window.is_null()).then_some(window)
}

/// The window procedure. It never holds the state across a call that runs a message loop of its own
/// (the menu's), which would dispatch back into it.
unsafe extern "system" fn procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_DRAIN => drain(window),
        // The icon's notification names the mouse message in the low word of `lparam`.
        WM_ICON if matches!((lparam & 0xFFFF) as u32, WM_LBUTTONUP | WM_RBUTTONUP) => {
            show_menu(window);
        }
        _ if Some(message) == taskbar_created() => {
            let view = STATE.with(|state| state.borrow().as_ref().and_then(|s| s.view.clone()));
            notify_icon(NIM_ADD, window, view.as_ref());
        }
        // SAFETY: every message this procedure does not handle goes to the default procedure as is.
        _ => return unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
    0
}

/// Explorer's restart message, once the state holds it.
fn taskbar_created() -> Option<u32> {
    STATE.with(|state| state.borrow().as_ref().map(|state| state.taskbar_created))
}

/// Do what the connection queued; end the message loop once the connection has ended.
fn drain(window: HWND) {
    loop {
        let next = STATE.with(|state| {
            state
                .borrow()
                .as_ref()
                .map(|state| state.commands.try_recv())
        });
        match next {
            Some(Ok(Command::Show(view))) => {
                notify_icon(NIM_MODIFY, window, Some(&view));
                STATE.with(|state| {
                    if let Some(state) = state.borrow_mut().as_mut() {
                        state.view = Some(view);
                    }
                });
            }
            Some(Ok(Command::Notify(notice))) => notify(&notice),
            Some(Err(TryRecvError::Empty)) | None => break,
            Some(Err(TryRecvError::Disconnected)) => {
                // SAFETY: called on the thread whose message loop is to end.
                unsafe { PostQuitMessage(0) };
                break;
            }
        }
    }
}

/// Show `notice` as a toast tagged with its key, which replaces the toast shown under the same tag. The
/// shortcut is looked for each time, so that one installed while the tray runs is used from then on.
fn notify(notice: &Notice) {
    let Some(report) =
        STATE.with(|state| state.borrow().as_ref().map(|state| state.report.clone()))
    else {
        return;
    };
    if !shortcut_installed() {
        report.set_notifications(Err(NO_APP_ID));
        return;
    }
    match toast(notice) {
        Ok(true) => report.set_notifications(Ok(())),
        Ok(false) => {
            tracing::info!(title = %notice.title, "toasts are turned off for the tray");
            report.set_notifications(Err(DISABLED));
        }
        Err(error) => {
            tracing::warn!(%error, title = %notice.title, "the toast could not be shown");
            report.set_notifications(Err(TOAST_FAILED));
        }
    }
}

/// Build and show the toast for `notice`; whether Windows lets it show, which it does not while
/// notifications are off for the app, the user or by policy. Its text is set as text nodes, never parsed
/// as XML.
fn toast(notice: &Notice) -> windows::core::Result<bool> {
    let document = XmlDocument::new()?;
    let toast = document.CreateElement(&HSTRING::from("toast"))?;
    let visual = document.CreateElement(&HSTRING::from("visual"))?;
    let binding = document.CreateElement(&HSTRING::from("binding"))?;
    binding.SetAttribute(&HSTRING::from("template"), &HSTRING::from("ToastGeneric"))?;
    for line in [&notice.title, &notice.body] {
        let text = document.CreateElement(&HSTRING::from("text"))?;
        text.SetInnerText(&HSTRING::from(line.as_str()))?;
        binding.AppendChild(&text)?;
    }
    visual.AppendChild(&binding)?;
    toast.AppendChild(&visual)?;
    document.AppendChild(&toast)?;

    let notification = ToastNotification::CreateToastNotification(&document)?;
    notification.SetTag(&HSTRING::from(notice.key.identifier()))?;
    let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))?;
    if notifier.Setting()? != NotificationSetting::Enabled {
        return Ok(false);
    }
    notifier.Show(&notification)?;

    Ok(true)
}

/// Whether the shortcut `keepster install` writes is in either `Programs` folder (ticket 68).
fn shortcut_installed() -> bool {
    shortcut_locations(|name| std::env::var_os(name))
        .iter()
        .any(|path| path.is_file())
}

/// Show the menu where the pointer is, and pass on what is chosen from it.
fn show_menu(window: HWND) {
    let Some(view) = STATE.with(|state| state.borrow().as_ref().and_then(|s| s.view.clone()))
    else {
        return;
    };
    let items = menu::items(&view);
    // SAFETY: a new, empty menu, destroyed below.
    let popup = unsafe { CreatePopupMenu() };
    if popup.is_null() {
        return;
    }
    for (index, item) in items.iter().enumerate() {
        let flags = if item.action.is_some() {
            MF_STRING
        } else {
            MF_STRING | MF_GRAYED
        };
        // SAFETY: `popup` is a live menu; the label is NUL-terminated and copied by the call. Command
        // ids start at 1, since `TrackPopupMenu` answers 0 for nothing chosen.
        unsafe { AppendMenuW(popup, flags, index + 1, to_wide(&item.label).as_ptr()) };
    }
    let mut pointer = POINT::default();
    // SAFETY: `pointer` is a valid out-pointer; the window and the menu are live. The window is brought
    // to the foreground first, and posted a null message after, so that the menu closes when the person
    // clicks elsewhere (as `TrackPopupMenu`'s documentation asks of a notification icon's menu).
    let chosen = unsafe {
        GetCursorPos(&mut pointer);
        SetForegroundWindow(window);
        let chosen = TrackPopupMenu(
            popup,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            pointer.x,
            pointer.y,
            0,
            window,
            ptr::null(),
        );
        PostMessageW(window, WM_NULL, 0, 0);
        DestroyMenu(popup);
        chosen
    };
    let action = usize::try_from(chosen)
        .ok()
        .and_then(|id| id.checked_sub(1))
        .and_then(|index| items.get(index))
        .and_then(|item| item.action);
    if let Some(action) = action {
        STATE.with(|state| {
            if let Some(state) = state.borrow().as_ref() {
                let _ = state.actions.send(action);
            }
        });
    }
}

/// Add, change or remove the icon, showing `view` (or, before the first, the application's icon and
/// name); whether the notification area accepted it.
fn notify_icon(message: u32, window: HWND, view: Option<&View>) -> bool {
    let (icon, tip) = match view {
        Some(view) => (stock_icon(view.icon), view.status.as_str()),
        None => (IDI_APPLICATION, strings::APP_NAME),
    };
    let mut data = NOTIFYICONDATAW {
        cbSize: u32::try_from(size_of::<NOTIFYICONDATAW>()).unwrap_or(u32::MAX),
        hWnd: window,
        uID: ICON_ID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_ICON,
        // SAFETY: a null instance with a stock icon's id loads that system icon, which needs no freeing.
        hIcon: unsafe { LoadIconW(ptr::null_mut(), icon) },
        ..Default::default()
    };
    // The tooltip is cut to what the field holds, keeping its terminating NUL.
    let room = data.szTip.len() - 1;
    for (slot, unit) in data.szTip.iter_mut().zip(tip.encode_utf16().take(room)) {
        *slot = unit;
    }
    // SAFETY: `data` is a fully initialised structure whose size is in `cbSize`.
    unsafe { Shell_NotifyIconW(message, &data) != 0 }
}

/// The system icon shown for `icon`.
fn stock_icon(icon: Icon) -> *const u16 {
    match icon {
        Icon::Covered => IDI_SHIELD,
        Icon::Attention => IDI_WARNING,
        Icon::Stopped => IDI_ERROR,
    }
}
