use std::{
    cell::Cell,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError, Sender},
        Mutex,
    },
    time::{Duration, Instant},
};

use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, Monitor, PhysicalPosition, PhysicalSize, Rect, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder, WindowEvent,
};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Dwm::{
        DwmExtendFrameIntoClientArea, DwmGetCompositionTimingInfo, DwmSetWindowAttribute,
        DWMNCRP_DISABLED, DWMWA_ALLOW_NCPAINT, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE,
        DWMWA_NCRENDERING_POLICY, DWMWA_TRANSITIONS_FORCEDISABLED, DWMWA_USE_IMMERSIVE_DARK_MODE,
        DWM_TIMING_INFO,
    },
    Graphics::Gdi::{ClientToScreen, CreateRoundRectRgn, DeleteObject, SetWindowRgn},
    System::LibraryLoader::{GetModuleHandleW, GetProcAddress},
    UI::{
        Controls::MARGINS,
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, GetCapture, ReleaseCapture, SetCapture, VK_LBUTTON, VK_RBUTTON,
        },
        Shell::{DefSubclassProc, GetWindowSubclass, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            GetCursorPos, GetSystemMetrics, GetWindowLongW, GetWindowRect, IsZoomed, KillTimer,
            PostMessageW, SetTimer, SetWindowLongW, SetWindowPos, GWL_EXSTYLE, GWL_STYLE, HTBOTTOM,
            HTBOTTOMLEFT, HTBOTTOMRIGHT, HTCAPTION, HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT,
            SM_SWAPBUTTON, STYLESTRUCT, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOCOPYBITS,
            SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WM_CANCELMODE, WM_CAPTURECHANGED, WM_LBUTTONUP,
            WM_MOUSEMOVE, WM_NCACTIVATE, WM_NCDESTROY, WM_NCLBUTTONDOWN, WM_NCPAINT,
            WM_STYLECHANGING, WM_TIMER, WS_CAPTION, WS_EX_CLIENTEDGE, WS_EX_DLGMODALFRAME,
            WS_EX_STATICEDGE, WS_EX_WINDOWEDGE, WS_SYSMENU,
        },
    },
};
use winreg::{
    enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY, KEY_WRITE},
    RegKey,
};

use crate::{
    clipboard,
    model::{ColorTheme, Material, Model, Settings},
};

const TRAY_ID: &str = "clipstack";
const QUICK_WIDTH: f64 = 406.0;
const QUICK_HEIGHT: f64 = 448.0;
const TOAST_WIDTH: f64 = 370.0;
const TOAST_HEIGHT: f64 = 64.0;
const PERSONALIZE: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const STARTUP_APPROVED: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
const PREF_KEY: &str = r"Software\cn.siyanghuasangou.clipstack";
const RUN_NAME: &str = "Clipstack.Win64";
const MACHINE_RUN_NAME: &str = "Clipstack.Win64.AllUsers";
pub const COPY_NOTICE: &str = "复制成功，按 Ctrl+V 粘贴";
pub const PICK_NOTICE: &str = "已选取，按 Ctrl+V 粘贴";
const NOTICE_DURATION: Duration = Duration::from_secs(3);
const NOTICE_EXIT_LEAD: Duration = Duration::from_millis(130);
const DARK_TRAY: &[u8] = include_bytes!("../icons/tray-dark.png");
const LIGHT_TRAY: &[u8] = include_bytes!("../icons/tray-light.png");

pub type AppModel = Mutex<Model>;

pub struct NoticeTimer(Sender<u64>);
#[derive(Default)]
pub struct SearchRequest(AtomicBool);
impl SearchRequest {
    pub fn take(&self) -> bool {
        self.0.swap(false, Ordering::AcqRel)
    }
}

struct WindowFrame {
    legacy_acrylic: Cell<bool>,
    drag: Cell<Option<DragFrame>>,
}

#[derive(Clone, Copy)]
struct DragFrame {
    hit: u32,
    anchor: POINT,
    start: RECT,
    minimum: PhysicalSize<i32>,
    interval: Duration,
    next_update: Instant,
    captured: bool,
}

#[derive(serde::Deserialize)]
pub struct PointerOrigin {
    x: i32,
    y: i32,
}

const MAIN_MIN_WIDTH: f64 = 720.0;
const MAIN_MIN_HEIGHT: f64 = 450.0;
const FRAME_SUBCLASS: usize = 1;
const DRAG_TIMER: usize = 0x4353;
const CAPTION_STYLE: u32 = WS_CAPTION | WS_SYSMENU;
const FRAME_EXSTYLE: u32 =
    WS_EX_WINDOWEDGE | WS_EX_CLIENTEDGE | WS_EX_STATICEDGE | WS_EX_DLGMODALFRAME;

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    app.manage(SearchRequest::default());
    let main = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("剪栈")
        .inner_size(1088.0, 668.0)
        .min_inner_size(MAIN_MIN_WIDTH, MAIN_MIN_HEIGHT)
        .center()
        .visible(false)
        .focused(false)
        .decorations(false)
        .shadow(false)
        .transparent(true)
        .build()?;
    let quick = WebviewWindowBuilder::new(
        app,
        "quick",
        WebviewUrl::App("index.html?window=quick".into()),
    )
    .title("剪栈 · 快捷收纳区")
    .inner_size(QUICK_WIDTH, QUICK_HEIGHT)
    .visible(false)
    .focused(false)
    .decorations(false)
    .shadow(false)
    .transparent(true)
    .minimizable(false)
    .maximizable(false)
    .resizable(false)
    .skip_taskbar(true)
    .always_on_top(true)
    .build()?;
    let toast = WebviewWindowBuilder::new(
        app,
        "toast",
        WebviewUrl::App("index.html?window=toast".into()),
    )
    .title("剪栈 · 通知")
    .inner_size(TOAST_WIDTH, TOAST_HEIGHT)
    .visible(false)
    .focused(false)
    .focusable(false)
    .decorations(false)
    .shadow(false)
    .transparent(true)
    .minimizable(false)
    .maximizable(false)
    .resizable(false)
    .skip_taskbar(true)
    .always_on_top(true)
    .build()?;
    toast.set_ignore_cursor_events(true)?;
    let preview = WebviewWindowBuilder::new(
        app,
        "preview",
        WebviewUrl::App("index.html?window=preview".into()),
    )
    .title("剪栈 · 快速预览")
    .inner_size(744.0, 540.0)
    .center()
    .visible(false)
    .focused(false)
    .decorations(false)
    .shadow(false)
    .transparent(true)
    .minimizable(false)
    .maximizable(false)
    .resizable(false)
    .skip_taskbar(true)
    .always_on_top(true)
    .build()?;
    for window in [&main, &quick, &toast, &preview] {
        install_custom_frame(window)?;
    }

    let window = preview.clone();
    preview.on_window_event(move |event| match event {
        WindowEvent::Focused(false) => {
            let _ = window.hide();
        }
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            let _ = window.hide();
        }
        _ => {}
    });

    let window = main.clone();
    let handle = app.clone();
    main.on_window_event(move |event| match event {
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            let _ = window.hide();
        }
        WindowEvent::ThemeChanged(_) => {
            let app = handle.clone();
            let _ = handle.run_on_main_thread(move || {
                let settings = app.state::<AppModel>().lock().unwrap().settings.clone();
                if settings.theme == ColorTheme::System {
                    apply_appearance(&app, &settings);
                }
                update_tray_icon(&app);
            });
        }
        _ => {}
    });
    let window = quick.clone();
    let handle = app.clone();
    quick.on_window_event(move |event| match event {
        WindowEvent::Focused(false) if !handle.state::<AppModel>().lock().unwrap().pinned => {
            let _ = window.hide();
        }
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            handle.state::<AppModel>().lock().unwrap().pinned = false;
            let _ = window.hide();
            state_changed(&handle);
        }
        _ => {}
    });

    let show = MenuItem::with_id(app, "show", "显示主界面", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(tray_image()?)
        .tooltip("剪栈")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => {
                let _ = show_main(app);
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } = event
            {
                let _ = toggle_quick(tray.app_handle(), Some(rect));
            }
        })
        .build(app)?;

    let (tx, rx) = mpsc::channel();
    app.manage(NoticeTimer(tx));
    let handle = app.clone();
    std::thread::Builder::new()
        .name("clipstack-notice".into())
        .spawn(move || {
            while let Ok(mut generation) = rx.recv() {
                let mut deadline = Instant::now() + NOTICE_DURATION;
                let mut exiting = false;
                loop {
                    let next_stage = if exiting {
                        deadline
                    } else {
                        deadline - NOTICE_EXIT_LEAD
                    };
                    match rx.recv_timeout(next_stage.saturating_duration_since(Instant::now())) {
                        Ok(next) => {
                            generation = next;
                            deadline = Instant::now() + NOTICE_DURATION;
                            exiting = false;
                        }
                        Err(RecvTimeoutError::Timeout) if !exiting => {
                            let app = handle.clone();
                            let _ = handle
                                .run_on_main_thread(move || animate_notice_exit(&app, generation));
                            exiting = true;
                        }
                        Err(RecvTimeoutError::Timeout) => {
                            let app = handle.clone();
                            let _ = handle
                                .run_on_main_thread(move || hide_notice(&app, Some(generation)));
                            break;
                        }
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
            }
        })?;
    let settings = app.state::<AppModel>().lock().unwrap().settings.clone();
    apply_appearance(app, &settings);
    Ok(())
}

fn install_custom_frame(window: &WebviewWindow) -> tauri::Result<()> {
    let hwnd = window.hwnd()?.0;
    let frame = Box::into_raw(Box::new(WindowFrame {
        legacy_acrylic: Cell::new(false),
        drag: Cell::new(None),
    }));
    // Tao retains these styles even for borderless windows. Remove the
    // caption, system menu and native edges before the window is first shown.
    unsafe {
        if SetWindowSubclass(
            hwnd,
            Some(custom_frame_proc),
            FRAME_SUBCLASS,
            frame as usize,
        ) == 0
        {
            drop(Box::from_raw(frame));
            return Err(std::io::Error::last_os_error().into());
        }
        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32 & !CAPTION_STYLE;
        let exstyle = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & !FRAME_EXSTYLE;
        SetWindowLongW(hwnd, GWL_STYLE, style as i32);
        SetWindowLongW(hwnd, GWL_EXSTYLE, exstyle as i32);
        set_dwm_attribute(hwnd, DWMWA_ALLOW_NCPAINT, 0);
        if !set_dwm_attribute(hwnd, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE) {
            set_dwm_attribute(hwnd, DWMWA_NCRENDERING_POLICY, DWMNCRP_DISABLED as u32);
            set_dwm_attribute(hwnd, DWMWA_TRANSITIONS_FORCEDISABLED, 1);
        }
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
        );
    }
    Ok(())
}

unsafe extern "system" fn custom_frame_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    subclass: usize,
    data: usize,
) -> LRESULT {
    // The subclass owns the boxed state until the HWND is destroyed. Its
    // Cell fields allow Win32 calls to re-enter the procedure on this thread.
    unsafe {
        if message == WM_NCDESTROY {
            KillTimer(hwnd, DRAG_TIMER);
            RemoveWindowSubclass(hwnd, Some(custom_frame_proc), subclass);
            let frame = Box::from_raw(data as *mut WindowFrame);
            let result = DefSubclassProc(hwnd, message, wparam, lparam);
            drop(frame);
            return result;
        }
        let frame = &*(data as *const WindowFrame);
        match message {
            WM_STYLECHANGING if lparam != 0 => {
                let change = &mut *(lparam as *mut STYLESTRUCT);
                match wparam as i32 {
                    GWL_STYLE => change.styleNew &= !CAPTION_STYLE,
                    GWL_EXSTYLE => change.styleNew &= !FRAME_EXSTYLE,
                    _ => {}
                }
            }
            WM_NCPAINT => return 0,
            WM_NCACTIVATE => {
                // Preserve Tao's focus events but stop DefWindowProc painting
                // a native caption when the window activates or deactivates.
                return DefSubclassProc(hwnd, message, wparam, -1);
            }
            WM_MOUSEMOVE if frame.drag.get().is_some() => {
                update_drag(hwnd, frame, false);
                return 0;
            }
            WM_LBUTTONUP if frame.drag.get().is_some() => {
                finish_drag(hwnd, frame, true);
                return 0;
            }
            WM_TIMER if wparam == DRAG_TIMER => {
                if primary_button_down() {
                    update_drag(hwnd, frame, false);
                } else {
                    finish_drag(hwnd, frame, true);
                }
                return 0;
            }
            WM_CAPTURECHANGED | WM_CANCELMODE if frame.drag.get().is_some() => {
                finish_drag(hwnd, frame, false);
            }
            _ => {}
        }
        DefSubclassProc(hwnd, message, wparam, lparam)
    }
}

fn set_dwm_attribute(hwnd: HWND, attribute: i32, value: u32) -> bool {
    unsafe { DwmSetWindowAttribute(hwnd, attribute as u32, (&value as *const u32).cast(), 4) >= 0 }
}

fn set_legacy_acrylic(hwnd: HWND, enabled: bool) {
    let mut data = 0;
    unsafe {
        if GetWindowSubclass(hwnd, Some(custom_frame_proc), FRAME_SUBCLASS, &mut data) != 0 {
            let frame = &*(data as *const WindowFrame);
            frame.legacy_acrylic.set(enabled);
        }
    }
}

fn refresh_interval() -> Duration {
    let mut timing = DWM_TIMING_INFO {
        cbSize: size_of::<DWM_TIMING_INFO>() as u32,
        ..Default::default()
    };
    if unsafe { DwmGetCompositionTimingInfo(std::ptr::null_mut(), &mut timing) } >= 0 {
        let rate = timing.rateRefresh;
        if rate.uiNumerator != 0 && rate.uiDenominator != 0 {
            return Duration::from_secs_f64(
                f64::from(rate.uiDenominator) / f64::from(rate.uiNumerator),
            )
            .clamp(Duration::from_millis(10), Duration::from_millis(34));
        }
    }
    Duration::from_millis(17)
}

fn primary_button_down() -> bool {
    unsafe {
        let button = if GetSystemMetrics(SM_SWAPBUTTON) == 0 {
            VK_LBUTTON
        } else {
            VK_RBUTTON
        };
        GetAsyncKeyState(i32::from(button)) < 0
    }
}

fn drag_bounds(drag: DragFrame, cursor: POINT) -> RECT {
    let dx = cursor.x - drag.anchor.x;
    let dy = cursor.y - drag.anchor.y;
    let mut next = drag.start;
    if drag.hit == HTCAPTION {
        next.left += dx;
        next.right += dx;
        next.top += dy;
        next.bottom += dy;
        return next;
    }
    if matches!(drag.hit, HTLEFT | HTTOPLEFT | HTBOTTOMLEFT) {
        next.left = (next.left + dx).min(next.right - drag.minimum.width);
    } else if matches!(drag.hit, HTRIGHT | HTTOPRIGHT | HTBOTTOMRIGHT) {
        next.right = (next.right + dx).max(next.left + drag.minimum.width);
    }
    if matches!(drag.hit, HTTOP | HTTOPLEFT | HTTOPRIGHT) {
        next.top = (next.top + dy).min(next.bottom - drag.minimum.height);
    } else if matches!(drag.hit, HTBOTTOM | HTBOTTOMLEFT | HTBOTTOMRIGHT) {
        next.bottom = (next.bottom + dy).max(next.top + drag.minimum.height);
    }
    next
}

fn update_drag(hwnd: HWND, frame: &WindowFrame, force: bool) {
    let Some(mut drag) = frame.drag.get() else {
        return;
    };
    let now = Instant::now();
    if !force && now < drag.next_update {
        return;
    }
    let mut cursor = POINT::default();
    let mut current = RECT::default();
    unsafe {
        if GetCursorPos(&mut cursor) == 0 || GetWindowRect(hwnd, &mut current) == 0 {
            return;
        }
        if !drag.captured {
            // Keep ordinary title-bar clicks and double-clicks inside WebView2.
            let threshold = if drag.hit == HTCAPTION { 3 } else { 1 };
            if force
                || (cursor.x - drag.anchor.x)
                    .abs()
                    .max((cursor.y - drag.anchor.y).abs())
                    < threshold
            {
                return;
            }
            SetCapture(hwnd);
            if GetCapture() != hwnd {
                return;
            }
            drag.captured = true;
            frame.drag.set(Some(drag));
        }
        let next = drag_bounds(drag, cursor);
        if (next.left, next.top, next.right, next.bottom)
            == (current.left, current.top, current.right, current.bottom)
        {
            return;
        }
        drag.next_update = now + drag.interval;
        frame.drag.set(Some(drag));
        let flags = SWP_NOACTIVATE
            | SWP_NOZORDER
            | if drag.hit == HTCAPTION {
                SWP_NOSIZE
            } else {
                SWP_NOCOPYBITS
            };
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            next.left,
            next.top,
            next.right - next.left,
            next.bottom - next.top,
            flags,
        );
    }
}

fn finish_drag(hwnd: HWND, frame: &WindowFrame, apply: bool) {
    if apply {
        update_drag(hwnd, frame, true);
    }
    frame.drag.set(None);
    unsafe {
        KillTimer(hwnd, DRAG_TIMER);
        if GetCapture() == hwnd {
            ReleaseCapture();
        }
    }
}

fn begin_captured_drag(
    hwnd: HWND,
    frame: &WindowFrame,
    hit: u32,
    anchor: POINT,
    minimum: PhysicalSize<i32>,
) -> bool {
    let mut start = RECT::default();
    unsafe {
        if GetWindowRect(hwnd, &mut start) == 0 {
            return false;
        }
        let interval = refresh_interval();
        if SetTimer(
            hwnd,
            DRAG_TIMER,
            interval.as_millis().div_ceil(2) as u32,
            None,
        ) == 0
        {
            return false;
        }
        frame.drag.set(Some(DragFrame {
            hit,
            anchor,
            start,
            minimum,
            interval,
            next_update: Instant::now(),
            captured: false,
        }));
        update_drag(hwnd, frame, false);
    }
    true
}

fn tray_image() -> tauri::Result<Image<'static>> {
    let bytes = if personalize("SystemUsesLightTheme", 0) != 0 {
        LIGHT_TRAY
    } else {
        DARK_TRAY
    };
    Image::from_bytes(bytes)
}

fn update_tray_icon(app: &AppHandle) {
    if let (Some(tray), Ok(icon)) = (app.tray_by_id(TRAY_ID), tray_image()) {
        let _ = tray.set_icon(Some(icon));
    }
}

pub fn state_changed(app: &AppHandle) {
    let _ = app.emit("state-changed", ());
}

pub fn show_main(app: &AppHandle) -> Result<(), String> {
    let main = get_window(app, "main")?;
    if let Some(quick) = app.get_webview_window("quick") {
        if !app.state::<AppModel>().lock().unwrap().pinned {
            let _ = quick.hide();
        }
    }
    let _ = main.emit("main-opened", ());
    main.unminimize()
        .and_then(|_| main.show())
        .and_then(|_| main.set_focus())
        .map_err(|error| error.to_string())
}

pub fn show_search(app: &AppHandle) -> Result<(), String> {
    app.state::<SearchRequest>()
        .0
        .store(true, Ordering::Release);
    show_main(app)?;
    let _ = get_window(app, "main")?.emit("search-opened", ());
    Ok(())
}

pub fn show_preview(app: &AppHandle, id: &str) -> Result<(), String> {
    let preview = get_window(app, "preview")?;
    if let Some(monitor) = app
        .cursor_position()
        .ok()
        .and_then(|point| monitor_at(app, point))
    {
        let area = monitor.work_area();
        let scale = monitor.scale_factor();
        let width = (744.0 * scale)
            .round()
            .min(f64::from(area.size.width.saturating_sub(16))) as u32;
        let height = (540.0 * scale)
            .round()
            .min(f64::from(area.size.height.saturating_sub(16))) as u32;
        let _ = preview.set_size(PhysicalSize::new(width.max(1), height.max(1)));
        let _ = preview.set_position(PhysicalPosition::new(
            area.position.x + ((area.size.width - width) / 2) as i32,
            area.position.y + ((area.size.height - height) / 2) as i32,
        ));
    }
    let _ = preview.emit("preview-opened", id);
    round_panel(&preview);
    preview
        .show()
        .and_then(|_| preview.set_focus())
        .map_err(|error| error.to_string())
}

fn get_window(app: &AppHandle, label: &str) -> Result<WebviewWindow, String> {
    app.get_webview_window(label)
        .ok_or_else(|| "窗口暂不可用。".into())
}

pub fn toggle_quick(app: &AppHandle, rect: Option<Rect>) -> Result<(), String> {
    let quick = get_window(app, "quick")?;
    if quick.is_visible().unwrap_or_default() {
        if app.state::<AppModel>().lock().unwrap().pinned {
            return quick.set_focus().map_err(|error| error.to_string());
        }
        return quick.hide().map_err(|error| error.to_string());
    }
    let rect = rect.or_else(|| {
        app.tray_by_id(TRAY_ID)
            .and_then(|tray| tray.rect().ok().flatten())
    });
    let center = rect.map(|rect| {
        let p = rect.position.to_physical::<f64>(1.0);
        let s = rect.size.to_physical::<f64>(1.0);
        PhysicalPosition::new(p.x + s.width / 2.0, p.y + s.height / 2.0)
    });
    let point = center
        .or_else(|| app.cursor_position().ok())
        .unwrap_or_default();
    if let Some(monitor) = monitor_at(app, point) {
        let area = monitor.work_area();
        let dpi = monitor.scale_factor();
        let gap = (8.0 * dpi).round() as i32;
        let width =
            ((QUICK_WIDTH * dpi).round() as i32).min((area.size.width as i32 - 2 * gap).max(1));
        let height =
            ((QUICK_HEIGHT * dpi).round() as i32).min((area.size.height as i32 - 2 * gap).max(1));
        let left = area.position.x + gap;
        let right = area.position.x + area.size.width as i32 - gap - width;
        let bottom = area.position.y + area.size.height as i32 - gap;
        let anchor = center
            .map(|point| point.x as i32)
            .unwrap_or(right + width / 2);
        let above = rect
            .map(|rect| rect.position.to_physical::<i32>(1.0).y - gap)
            .unwrap_or(bottom);
        let y = (bottom.min(above) - height).clamp(
            area.position.y + gap,
            (bottom - height).max(area.position.y + gap),
        );
        let _ = quick.set_position(PhysicalPosition::new(
            (anchor - width / 2).clamp(left, right.max(left)),
            y,
        ));
        let _ = quick.set_size(PhysicalSize::new(width as u32, height as u32));
    }
    let _ = quick.emit("quick-opened", ());
    round_panel(&quick);
    quick
        .show()
        .and_then(|_| quick.set_focus())
        .map_err(|error| error.to_string())
}

fn monitor_at(app: &AppHandle, point: PhysicalPosition<f64>) -> Option<Monitor> {
    app.monitor_from_point(point.x, point.y)
        .ok()
        .flatten()
        .or_else(|| app.primary_monitor().ok().flatten())
}

pub fn show_notice(app: &AppHandle, message: &'static str) {
    let Some(toast) = app.get_webview_window("toast") else {
        return;
    };
    let generation = {
        let state = app.state::<AppModel>();
        let mut model = state.lock().unwrap();
        if !model.settings.notifications {
            return;
        }
        model.notice_generation = model.notice_generation.wrapping_add(1);
        model.notice = Some(message);
        model.notice_generation
    };
    let point = app.cursor_position().unwrap_or_default();
    if let Some(monitor) = monitor_at(app, point) {
        let area = monitor.work_area();
        let dpi = monitor.scale_factor();
        let width = (TOAST_WIDTH * dpi).round() as i32;
        let height = (TOAST_HEIGHT * dpi).round() as i32;
        let x = area.position.x + (area.size.width as i32 - width) / 2;
        let y = area.position.y + area.size.height as i32 - height - (28.0 * dpi).round() as i32;
        let _ = toast.set_position(PhysicalPosition::new(x, y));
        let _ = toast.set_size(PhysicalSize::new(width as u32, height as u32));
    }
    let _ = toast.emit("clipboard-notice", message);
    round_panel(&toast);
    let _ = toast.show();
    clipboard::set_paste_watch(true);
    let _ = app.state::<NoticeTimer>().0.send(generation);
}

pub fn hide_notice(app: &AppHandle, generation: Option<u64>) {
    {
        let state = app.state::<AppModel>();
        let mut model = state.lock().unwrap();
        if generation.is_some_and(|generation| generation != model.notice_generation) {
            return;
        }
        model.notice = None;
    }
    clipboard::set_paste_watch(false);
    if let Some(toast) = app.get_webview_window("toast") {
        let _ = toast.hide();
    }
}

fn animate_notice_exit(app: &AppHandle, generation: u64) {
    let current = {
        let state = app.state::<AppModel>();
        let model = state.lock().unwrap();
        model.notice_generation == generation && model.notice.is_some()
    };
    if current {
        if let Some(toast) = app.get_webview_window("toast") {
            let _ = toast.emit("clipboard-notice-exit", ());
        }
    }
}

fn personalize(name: &str, default: u32) -> u32 {
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(PERSONALIZE)
        .and_then(|key| key.get_value(name))
        .unwrap_or(default)
}

fn round_panel(window: &WebviewWindow) {
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND};
    let Ok(handle) = window.hwnd() else {
        return;
    };
    let hwnd = handle.0;
    if set_dwm_attribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND as u32) {
        return;
    }
    let mut bounds = RECT::default();
    let radius = (window.scale_factor().unwrap_or(1.0) * 16.0).round() as i32;
    unsafe {
        if GetWindowRect(hwnd, &mut bounds) == 0 {
            return;
        }
        let region = CreateRoundRectRgn(
            0,
            0,
            bounds.right - bounds.left + 1,
            bounds.bottom - bounds.top + 1,
            radius,
            radius,
        );
        if !region.is_null() && SetWindowRgn(hwnd, region, 1) == 0 {
            DeleteObject(region);
        }
    }
}

pub fn apply_appearance(app: &AppHandle, settings: &Settings) {
    let dark = match settings.theme {
        ColorTheme::System => personalize("AppsUseLightTheme", 1) == 0,
        ColorTheme::Dark => true,
        ColorTheme::Light => false,
    };
    let tint = if dark {
        (23, 35, 43, 178)
    } else {
        (224, 236, 240, 185)
    };
    let transparency = personalize("EnableTransparency", 1) != 0;
    let mut native = true;
    for window in app.webview_windows().values() {
        // Set the compositor color directly. Tao's set_theme path asks
        // DefWindowProc to repaint the standard title bar synchronously.
        let Ok(handle) = window.hwnd() else {
            native = false;
            continue;
        };
        let hwnd = handle.0;
        set_dwm_attribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, u32::from(dark));
        let dwm_backdrop = set_dwm_attribute(hwnd, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE);
        let _ = window_vibrancy::clear_mica(window);
        if dwm_backdrop {
            let _ = window_vibrancy::clear_acrylic(window);
            // Extend the Windows 11 compositor backdrop across the client.
            let margins = MARGINS {
                cxLeftWidth: -1,
                cxRightWidth: -1,
                cyTopHeight: -1,
                cyBottomHeight: -1,
            };
            unsafe {
                DwmExtendFrameIntoClientArea(hwnd, &margins);
            }
        } else {
            apply_legacy_accent(hwnd, None);
        }
        let mut legacy_acrylic = false;
        let applied = if !transparency {
            false
        } else if dwm_backdrop
            && settings.material == Material::Mica
            && window_vibrancy::apply_mica(window, Some(dark)).is_ok()
        {
            true
        } else {
            use windows_sys::Win32::Graphics::Dwm::{
                DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE,
            };
            if dwm_backdrop
                && set_dwm_attribute(
                    hwnd,
                    DWMWA_SYSTEMBACKDROP_TYPE,
                    DWMSBT_TRANSIENTWINDOW as u32,
                )
            {
                true
            } else {
                legacy_acrylic = apply_legacy_accent(hwnd, Some(tint));
                legacy_acrylic
            }
        };
        set_legacy_acrylic(hwnd, legacy_acrylic && window.label() == "main");
        native &= applied;
    }
    app.state::<AppModel>().lock().unwrap().native_backdrop = native;
    state_changed(app);
}

fn apply_legacy_accent(hwnd: HWND, tint: Option<(u8, u8, u8, u8)>) -> bool {
    #[repr(C)]
    struct AccentPolicy {
        state: u32,
        flags: u32,
        tint: u32,
        animation: u32,
    }
    #[repr(C)]
    struct CompositionAttribute {
        attribute: u32,
        data: *const AccentPolicy,
        size: usize,
    }
    type SetAttribute = unsafe extern "system" fn(HWND, *const CompositionAttribute) -> i32;
    const USER32: &[u16] = &[117, 115, 101, 114, 51, 50, 46, 100, 108, 108, 0];
    static SET_ATTRIBUTE: std::sync::OnceLock<Option<SetAttribute>> = std::sync::OnceLock::new();
    let Some(set_attribute) = *SET_ATTRIBUTE.get_or_init(|| unsafe {
        let module = GetModuleHandleW(USER32.as_ptr());
        if module.is_null() {
            return None;
        }
        GetProcAddress(module, c"SetWindowCompositionAttribute".as_ptr().cast())
            .map(|proc| std::mem::transmute::<_, SetAttribute>(proc))
    }) else {
        return false;
    };
    // WCA_ACCENT_POLICY (19): ACCENT_ENABLE_ACRYLICBLURBEHIND (4) with an ABGR tint.
    let policy = AccentPolicy {
        state: if tint.is_some() { 4 } else { 0 },
        flags: 0,
        tint: tint
            .map(|(r, g, b, a)| u32::from_le_bytes([r, g, b, a]))
            .unwrap_or(0),
        animation: 0,
    };
    let attribute = CompositionAttribute {
        attribute: 19,
        data: &policy,
        size: size_of::<AccentPolicy>(),
    };
    unsafe { set_attribute(hwnd, &attribute) != 0 }
}

pub fn set_autostart(enabled: bool) -> Result<(), String> {
    set_autostart_registration(enabled, true)
}

pub fn set_install_user_autostart(enabled: bool) -> Result<(), String> {
    set_autostart_registration(enabled, false)
}

fn set_autostart_registration(enabled: bool, allow_machine: bool) -> Result<(), String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (run, _) = hkcu.create_subkey(RUN_KEY).map_err(autostart_error)?;
    let (prefs, _) = hkcu.create_subkey(PREF_KEY).map_err(autostart_error)?;
    let command = autostart_command().map_err(autostart_error)?;
    if enabled && !(allow_machine && machine_autostart(&hkcu, &command)) {
        run.set_value(RUN_NAME, &command).map_err(autostart_error)?;
        if let Ok(approval) = hkcu.open_subkey_with_flags(STARTUP_APPROVED, KEY_READ | KEY_WRITE) {
            if startup_blocked(&approval, RUN_NAME) {
                approval.delete_value(RUN_NAME).map_err(autostart_error)?;
            }
        }
    } else if let Err(error) = run.delete_value(RUN_NAME) {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(autostart_error(error));
        }
    }
    prefs
        .set_value("Autostart", &if enabled { "on" } else { "off" })
        .map_err(autostart_error)
}

pub fn remove_user_autostart() -> Result<(), String> {
    let result = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(RUN_KEY, KEY_WRITE)
        .and_then(|key| key.delete_value(RUN_NAME));
    match result {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(autostart_error(error)),
        _ => Ok(()),
    }
}

pub fn report_installer_result(token: &str, success: bool) -> std::io::Result<()> {
    let (prefs, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(PREF_KEY)?;
    let status = if success { "ok" } else { "error" };
    prefs.set_value("InstallerResult", &format!("{token}:{status}"))
}

fn autostart_error(error: std::io::Error) -> String {
    format!("无法更新开机启动设置：{error}")
}

fn startup_blocked(key: &RegKey, name: &str) -> bool {
    key.get_raw_value(name)
        .ok()
        .and_then(|v| v.bytes.first().copied())
        .is_some_and(|status| matches!(status, 3 | 7))
}

pub fn autostart_enabled() -> bool {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if hkcu
        .open_subkey(PREF_KEY)
        .and_then(|key| key.get_value::<String, _>("Autostart"))
        .is_ok_and(|value| value == "off")
    {
        return false;
    }
    let Ok(command) = autostart_command() else {
        return false;
    };
    approved_registration(&hkcu, RUN_NAME, &command) || machine_autostart(&hkcu, &command)
}

fn autostart_command() -> std::io::Result<String> {
    std::env::current_exe().map(|path| format!("\"{}\" --autostart", path.display()))
}

fn approved_registration(hive: &RegKey, name: &str, command: &str) -> bool {
    hive.open_subkey_with_flags(RUN_KEY, KEY_READ | KEY_WOW64_64KEY)
        .and_then(|key| key.get_value::<String, _>(name))
        .is_ok_and(|value| value.eq_ignore_ascii_case(command))
        && !hive
            .open_subkey_with_flags(STARTUP_APPROVED, KEY_READ | KEY_WOW64_64KEY)
            .is_ok_and(|key| startup_blocked(&key, name))
}

fn machine_autostart(hkcu: &RegKey, command: &str) -> bool {
    approved_registration(
        &RegKey::predef(HKEY_LOCAL_MACHINE),
        MACHINE_RUN_NAME,
        command,
    ) && !hkcu
        .open_subkey(STARTUP_APPROVED)
        .is_ok_and(|key| startup_blocked(&key, MACHINE_RUN_NAME))
}

pub fn window_action(
    window: &WebviewWindow,
    action: &str,
    origin: Option<PointerOrigin>,
) -> Result<(), String> {
    let result = match action {
        "close" => window.close(),
        "minimize" => window.minimize(),
        "maximize" => {
            if window.is_maximized().unwrap_or(false) {
                window.unmaximize()
            } else {
                window.maximize()
            }
        }
        "drag" => return begin_move_or_resize(window, HTCAPTION, origin),
        "resize-n" => return begin_move_or_resize(window, HTTOP, origin),
        "resize-ne" => return begin_move_or_resize(window, HTTOPRIGHT, origin),
        "resize-e" => return begin_move_or_resize(window, HTRIGHT, origin),
        "resize-se" => return begin_move_or_resize(window, HTBOTTOMRIGHT, origin),
        "resize-s" => return begin_move_or_resize(window, HTBOTTOM, origin),
        "resize-sw" => return begin_move_or_resize(window, HTBOTTOMLEFT, origin),
        "resize-w" => return begin_move_or_resize(window, HTLEFT, origin),
        "resize-nw" => return begin_move_or_resize(window, HTTOPLEFT, origin),
        _ => return Err("未知窗口操作。".into()),
    };
    result.map_err(|error| error.to_string())
}

fn begin_move_or_resize(
    window: &WebviewWindow,
    hit: u32,
    origin: Option<PointerOrigin>,
) -> Result<(), String> {
    if window.label() != "main" {
        return Ok(());
    }
    let hwnd = window.hwnd().map_err(|error| error.to_string())?.0;
    let scale = window.scale_factor().map_err(|error| error.to_string())?;
    let minimum = PhysicalSize::new(
        (MAIN_MIN_WIDTH * scale).ceil() as i32,
        (MAIN_MIN_HEIGHT * scale).ceil() as i32,
    );
    let mut anchor = POINT::default();
    unsafe {
        if !primary_button_down() {
            return Ok(());
        }
        if let Some(origin) = origin {
            anchor = POINT {
                x: origin.x,
                y: origin.y,
            };
            if ClientToScreen(hwnd, &mut anchor) == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
        } else if GetCursorPos(&mut anchor) == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let mut data = 0;
        if IsZoomed(hwnd) == 0
            && GetWindowSubclass(hwnd, Some(custom_frame_proc), FRAME_SUBCLASS, &mut data) != 0
        {
            let frame = &*(data as *const WindowFrame);
            if frame.legacy_acrylic.get() && begin_captured_drag(hwnd, frame, hit, anchor, minimum)
            {
                return Ok(());
            }
        }
        let point = u32::from(anchor.x as i16 as u16) | (u32::from(anchor.y as i16 as u16) << 16);
        ReleaseCapture();
        if PostMessageW(hwnd, WM_NCLBUTTONDOWN, hit as usize, point as isize) == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(hit: u32) -> DragFrame {
        DragFrame {
            hit,
            anchor: POINT { x: -960, y: 250 },
            start: RECT {
                left: -1000,
                top: 200,
                right: 80,
                bottom: 850,
            },
            minimum: PhysicalSize::new(720, 450),
            interval: Duration::from_millis(16),
            next_update: Instant::now(),
            captured: true,
        }
    }

    fn edges(rect: RECT) -> [i32; 4] {
        [rect.left, rect.top, rect.right, rect.bottom]
    }

    #[test]
    fn absolute_motion_keeps_the_pointer_offset_even_when_updates_are_skipped() {
        let drag = frame(HTCAPTION);
        assert_eq!(
            edges(drag_bounds(drag, POINT { x: -959, y: 254 })),
            [-999, 204, 81, 854]
        );
        assert_eq!(
            edges(drag_bounds(drag, POINT { x: 440, y: 870 })),
            [400, 820, 1480, 1470]
        );
        assert_eq!(edges(drag_bounds(drag, drag.anchor)), edges(drag.start));
    }

    #[test]
    fn resize_moves_only_the_grabbed_edges_and_clamps_the_minimum() {
        let inward = POINT { x: -210, y: 720 };
        assert_eq!(
            edges(drag_bounds(frame(HTTOPLEFT), inward)),
            [-640, 400, 80, 850]
        );
        assert_eq!(
            edges(drag_bounds(frame(HTBOTTOM), inward)),
            [-1000, 200, 80, 1320]
        );
        let outward = POINT { x: -1080, y: 150 };
        assert_eq!(
            edges(drag_bounds(frame(HTTOPLEFT), outward)),
            [-1120, 100, 80, 850]
        );
        assert_eq!(
            edges(drag_bounds(frame(HTBOTTOMRIGHT), outward)),
            [-1000, 200, -40, 750]
        );
    }
}
