use std::{
    mem::size_of,
    ptr::null_mut,
    sync::{mpsc, Mutex},
    thread,
    time::{Duration, Instant},
};

use tauri::AppHandle;
use windows_sys::Win32::{
    Foundation::{HWND, RECT},
    System::DataExchange::GetClipboardSequenceNumber,
    UI::{
        Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK},
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
            KEYEVENTF_KEYUP, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
        },
        WindowsAndMessaging::{
            GetAncestor, GetClassNameW, GetForegroundWindow, GetGUIThreadInfo, GetShellWindow,
            GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
            SetForegroundWindow, EVENT_SYSTEM_FOREGROUND, GA_ROOT, GUITHREADINFO, GUI_INMENUMODE,
            GUI_POPUPMENUMODE, GUI_SYSTEMMENUMODE, WINEVENT_OUTOFCONTEXT,
        },
    },
};

pub const INPUT_TAG: usize = 0x434C4950;
const KEY_V: u16 = b'V' as u16;
const MODIFIER_TIMEOUT: Duration = Duration::from_millis(1500);
const FOCUS_TIMEOUT: Duration = Duration::from_secs(1);
const FOCUS_SETTLE: Duration = Duration::from_millis(40);
const POLL_INTERVAL: Duration = Duration::from_millis(8);
static LAST_EXTERNAL: Mutex<Option<Target>> = Mutex::new(None);

#[derive(Clone, Copy)]
pub struct Target {
    window: usize,
    thread: u32,
    process: u32,
}

impl Target {
    fn hwnd(self) -> HWND {
        self.window as HWND
    }

    fn is_current(self) -> bool {
        let mut process = 0;
        let thread = unsafe { GetWindowThreadProcessId(self.hwnd(), &mut process) };
        thread == self.thread && process == self.process
    }

    pub fn center(self) -> Option<(f64, f64)> {
        let mut bounds = RECT::default();
        (unsafe { GetWindowRect(self.hwnd(), &mut bounds) } != 0).then(|| {
            (
                (f64::from(bounds.left) + f64::from(bounds.right)) / 2.0,
                (f64::from(bounds.top) + f64::from(bounds.bottom)) / 2.0,
            )
        })
    }
}

fn is_shell_surface(hwnd: HWND) -> bool {
    let mut name = [0; 64];
    let length = unsafe { GetClassNameW(hwnd, name.as_mut_ptr(), name.len() as i32) } as usize;
    hwnd == unsafe { GetShellWindow() }
        || matches!(
            String::from_utf16_lossy(&name[..length]).as_str(),
            "Shell_TrayWnd"
                | "Shell_SecondaryTrayWnd"
                | "NotifyIconOverflowWindow"
                | "TopLevelWindowForOverflowXamlIsland"
                | "Progman"
                | "WorkerW"
        )
}

fn remember(hwnd: HWND) {
    let mut process = 0;
    let thread = unsafe { GetWindowThreadProcessId(hwnd, &mut process) };
    if thread != 0 && process != std::process::id() && !is_shell_surface(hwnd) {
        *LAST_EXTERNAL.lock().unwrap() = Some(Target {
            window: hwnd as usize,
            thread,
            process,
        });
    }
}

pub fn remember_foreground() {
    remember(unsafe { GetForegroundWindow() });
}

unsafe extern "system" fn foreground_changed(
    _: HWINEVENTHOOK,
    _: u32,
    hwnd: HWND,
    _: i32,
    _: i32,
    _: u32,
    _: u32,
) {
    remember(hwnd);
}

pub struct ForegroundHook(HWINEVENTHOOK);

impl ForegroundHook {
    pub fn install() -> Self {
        remember_foreground();
        Self(unsafe {
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                null_mut(),
                Some(foreground_changed),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        })
    }
}

impl Drop for ForegroundHook {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { UnhookWinEvent(self.0) };
        }
    }
}

pub fn last_target() -> Option<Target> {
    let target = (*LAST_EXTERNAL.lock().unwrap())?;
    let hwnd = target.hwnd();
    (target.is_current() && unsafe { IsIconic(hwnd) == 0 && IsWindowVisible(hwnd) != 0 })
        .then_some(target)
}

fn focused_control(hwnd: HWND, thread: u32) -> Option<HWND> {
    let mut gui = GUITHREADINFO {
        cbSize: size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        (GetGUIThreadInfo(thread, &mut gui) != 0
            && gui.hwndActive == hwnd
            && gui.flags & (GUI_INMENUMODE | GUI_POPUPMENUMODE | GUI_SYSTEMMENUMODE) == 0
            && GetAncestor(gui.hwndFocus, GA_ROOT) == hwnd)
            .then_some(gui.hwndFocus)
    }
}

pub fn external_focus() -> bool {
    let hwnd = unsafe { GetForegroundWindow() };
    let mut process = 0;
    let thread = unsafe { GetWindowThreadProcessId(hwnd, &mut process) };
    thread != 0
        && process != std::process::id()
        && !is_shell_surface(hwnd)
        && focused_control(hwnd, thread).is_some()
}

pub fn key_down(key: u16) -> bool {
    unsafe { GetAsyncKeyState(i32::from(key)) < 0 }
}

fn modifiers_released() -> bool {
    [VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN, KEY_V]
        .into_iter()
        .all(|key| !key_down(key))
}

fn paste_foreground(target: Target, sequence: u32) -> Option<HWND> {
    let foreground = unsafe { GetForegroundWindow() };
    let mut process = 0;
    unsafe { GetWindowThreadProcessId(foreground, &mut process) };
    (unsafe { GetClipboardSequenceNumber() } == sequence
        && target.is_current()
        && (foreground == target.hwnd() || process == std::process::id()))
    .then_some(foreground)
}

pub fn send_paste(app: &AppHandle, target: Target, sequence: u32) -> bool {
    // A hotkey's Alt and Ctrl releases belong to the picker. Word can enter
    // ribbon navigation when the original window receives the tail of that chord.
    let deadline = Instant::now() + MODIFIER_TIMEOUT;
    while !modifiers_released() {
        if Instant::now() >= deadline || paste_foreground(target, sequence).is_none() {
            return false;
        }
        thread::sleep(POLL_INTERVAL);
    }

    // Request activation from the UI thread, which received the user's click.
    let deadline = Instant::now() + FOCUS_TIMEOUT;
    let (reply, activated) = mpsc::channel();
    if app
        .run_on_main_thread(move || {
            let success = Instant::now() < deadline
                && modifiers_released()
                && paste_foreground(target, sequence).is_some_and(|foreground| {
                    foreground == target.hwnd()
                        || unsafe { SetForegroundWindow(target.hwnd()) } != 0
                });
            let _ = reply.send(success);
        })
        .is_err()
        || !activated.recv_timeout(FOCUS_TIMEOUT).unwrap_or(false)
    {
        return false;
    }

    // A foreground HWND can appear before Office has restored its editing focus.
    let mut settled_focus = None;
    loop {
        let Some(foreground) = paste_foreground(target, sequence) else {
            return false;
        };
        let focus = (foreground == target.hwnd() && modifiers_released())
            .then(|| focused_control(foreground, target.thread))
            .flatten();
        let ready = if let Some(control) = focus {
            let (previous, since) = settled_focus.get_or_insert_with(|| (control, Instant::now()));
            if *previous != control {
                (*previous, *since) = (control, Instant::now());
            }
            since.elapsed() >= FOCUS_SETTLE
        } else {
            settled_focus = None;
            false
        };
        if ready {
            let input = [
                key_event(VK_CONTROL, 0),
                key_event(KEY_V, 0),
                key_event(KEY_V, KEYEVENTF_KEYUP),
                key_event(VK_CONTROL, KEYEVENTF_KEYUP),
            ];
            return unsafe {
                SendInput(
                    input.len() as u32,
                    input.as_ptr(),
                    size_of::<INPUT>() as i32,
                )
            } == input.len() as u32;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn key_event(key: u16, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                dwFlags: flags,
                dwExtraInfo: INPUT_TAG,
                ..Default::default()
            },
        },
    }
}
