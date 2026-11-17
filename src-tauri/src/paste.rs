use std::{
    mem::size_of,
    ptr::null_mut,
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

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
            SetForegroundWindow, EVENT_SYSTEM_FOREGROUND, GA_ROOT, GUITHREADINFO,
            WINEVENT_OUTOFCONTEXT,
        },
    },
};

pub const INPUT_TAG: usize = 0x434C4950;
const KEY_V: u16 = b'V' as u16;
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

pub fn activate_target() -> Option<Target> {
    let target = (*LAST_EXTERNAL.lock().unwrap())?;
    let hwnd = target.hwnd();
    let foreground = unsafe { GetForegroundWindow() };
    let mut foreground_process = 0;
    unsafe { GetWindowThreadProcessId(foreground, &mut foreground_process) };
    if !target.is_current()
        || unsafe { IsIconic(hwnd) != 0 || IsWindowVisible(hwnd) == 0 || IsIconic(foreground) != 0 }
        || (foreground != hwnd && foreground_process != std::process::id())
    {
        return None;
    }
    (foreground == hwnd || unsafe { SetForegroundWindow(hwnd) } != 0).then_some(target)
}

fn focused_control(hwnd: HWND, thread: u32) -> bool {
    let mut gui = GUITHREADINFO {
        cbSize: size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        GetGUIThreadInfo(thread, &mut gui) != 0
            && gui.hwndActive == hwnd
            && GetAncestor(gui.hwndFocus, GA_ROOT) == hwnd
    }
}

pub fn external_focus() -> bool {
    let hwnd = unsafe { GetForegroundWindow() };
    let mut process = 0;
    let thread = unsafe { GetWindowThreadProcessId(hwnd, &mut process) };
    thread != 0
        && process != std::process::id()
        && !is_shell_surface(hwnd)
        && focused_control(hwnd, thread)
}

pub fn key_down(key: u16) -> bool {
    unsafe { GetAsyncKeyState(i32::from(key)) < 0 }
}

fn modifiers_released() -> bool {
    [VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN, KEY_V]
        .into_iter()
        .all(|key| !key_down(key))
}

pub fn send_paste(target: Target, sequence: u32) -> bool {
    // Foreground activation on a different GUI thread is asynchronous.
    let deadline = Instant::now() + Duration::from_millis(250);
    let hwnd = target.hwnd();
    loop {
        let foreground = unsafe { GetForegroundWindow() };
        let mut process = 0;
        unsafe { GetWindowThreadProcessId(foreground, &mut process) };
        if unsafe { GetClipboardSequenceNumber() } != sequence
            || !target.is_current()
            || (foreground != hwnd && process != std::process::id())
        {
            return false;
        }
        if foreground == hwnd && focused_control(hwnd, target.thread) && modifiers_released() {
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
        thread::sleep(Duration::from_millis(5));
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
