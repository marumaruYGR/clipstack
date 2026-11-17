use image::codecs::{
    bmp::BmpDecoder,
    png::{PngDecoder, PngEncoder},
};
use image::{DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, Limits};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::ffi::c_void;
use std::io::{self, Cursor};
use std::mem::size_of;
use std::ptr::{null, null_mut};
use std::rc::Rc;
use std::slice;
use std::sync::{
    atomic::{AtomicPtr, Ordering},
    mpsc,
};
use std::thread;
use std::time::Duration;
use windows_sys::Win32::{
    Foundation::{GlobalFree, HGLOBAL, HWND, LPARAM, LRESULT, WPARAM},
    Graphics::Gdi::{BITMAPV5HEADER, BI_BITFIELDS, BI_RGB, LCS_GM_IMAGES},
    System::{
        DataExchange::{
            AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData,
            GetClipboardOwner, GetClipboardSequenceNumber, IsClipboardFormatAvailable,
            OpenClipboard, RegisterClipboardFormatW, RemoveClipboardFormatListener,
            SetClipboardData,
        },
        LibraryLoader::GetModuleHandleW,
        Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE},
        Ole::{CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT, DROPEFFECT_COPY},
    },
    UI::{
        Input::KeyboardAndMouse::{VK_CONTROL, VK_INSERT, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT},
        Shell::{DragQueryFileW, DROPFILES},
        WindowsAndMessaging::{
            CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
            GetMessageW, KillTimer, PostMessageW, PostQuitMessage, RegisterClassW, SetTimer,
            SetWindowsHookExW, UnhookWindowsHookEx, UnregisterClassW, HHOOK, KBDLLHOOKSTRUCT, MSG,
            WH_KEYBOARD_LL, WM_APP, WM_CLIPBOARDUPDATE, WM_DESTROY, WM_ENDSESSION, WM_KEYDOWN,
            WM_QUERYENDSESSION, WM_TIMER, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
        },
    },
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Content {
    Text {
        text: String,
    },
    Link {
        text: String,
    },
    RichText {
        text: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "optional_base64"
        )]
        html: Option<Vec<u8>>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "optional_base64"
        )]
        rtf: Option<Vec<u8>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
    },
    Image {
        #[serde(with = "bytes_base64")]
        png: Vec<u8>,
        width: u32,
        height: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rich: Option<AccompanyingText>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bitmap: Option<NativeBitmap>,
    },
    Files {
        paths: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rich: Option<AccompanyingText>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccompanyingText {
    pub text: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "optional_base64"
    )]
    pub html: Option<Vec<u8>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "optional_base64"
    )]
    pub rtf: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeBitmap {
    version5: bool,
    #[serde(with = "bytes_base64")]
    bytes: Vec<u8>,
}

impl AccompanyingText {
    fn into_content(self) -> Content {
        let Self {
            text,
            html,
            rtf,
            url,
        } = self;
        match url {
            url if html.is_some() || rtf.is_some() => Content::RichText {
                text,
                html,
                rtf,
                url,
            },
            Some(url) if text.is_empty() || url == text => Content::Link { text: url },
            Some(url) => Content::RichText {
                text,
                html,
                rtf,
                url: Some(url),
            },
            None => Content::Text { text },
        }
    }
}

mod optional_base64 {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(
        bytes: &Option<Vec<u8>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        bytes
            .as_ref()
            .map(|value| STANDARD.encode(value))
            .serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Vec<u8>>, D::Error> {
        Option::<String>::deserialize(deserializer)?
            .map(|value| STANDARD.decode(value).map_err(serde::de::Error::custom))
            .transpose()
    }
}

mod bytes_base64 {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        STANDARD
            .decode(String::deserialize(deserializer)?)
            .map_err(serde::de::Error::custom)
    }
}

const WM_WATCH_PASTE: u32 = WM_APP + 1;
const WM_PASTE_CHORD: u32 = WM_APP + 2;
const READ_TIMER: usize = 2;
const READ_RETRIES: u8 = 12;
static LISTENER: AtomicPtr<c_void> = AtomicPtr::new(null_mut());

struct RegisteredFormats {
    png: u32,
    html: u32,
    rtf: u32,
    url: u32,
}

impl RegisteredFormats {
    fn register() -> Self {
        Self {
            png: register_format("PNG"),
            html: register_format("HTML Format"),
            rtf: register_format("Rich Text Format"),
            url: register_format("UniformResourceLocatorW"),
        }
    }
}

thread_local! {
    static SESSION_END: RefCell<Option<Rc<dyn Fn()>>> = RefCell::new(None);
}

pub fn start(
    on_change: impl Fn(Content) + Send + 'static,
    on_paste: impl Fn(u32, bool) + Send + 'static,
    on_session_end: impl Fn() + Send + 'static,
) -> Result<(), String> {
    let (ready, result) = mpsc::sync_channel(0);
    thread::Builder::new()
        .name("clipstack-clipboard".into())
        .spawn(move || {
            let window = match ListenerWindow::create() {
                Ok(window) => window,
                Err(error) => {
                    let _ = ready.send(Err(error));
                    return;
                }
            };
            let _foreground_hook = crate::paste::ForegroundHook::install();
            SESSION_END.with(|slot| *slot.borrow_mut() = Some(Rc::new(on_session_end)));
            LISTENER.store(window.hwnd, Ordering::Release);
            if ready.send(Ok(())).is_ok() {
                run_listener(window.hwnd, on_change, on_paste);
            }
            LISTENER.store(null_mut(), Ordering::Release);
            SESSION_END.with(|slot| *slot.borrow_mut() = None);
        })
        .map_err(|error| format!("无法启动剪贴板监听线程：{error}"))?;
    result
        .recv()
        .map_err(|_| "剪贴板监听线程未能启动".to_string())?
}

pub fn set_paste_watch(active: bool) {
    let hwnd = LISTENER.load(Ordering::Acquire);
    if !hwnd.is_null() {
        unsafe { PostMessageW(hwnd, WM_WATCH_PASTE, usize::from(active), 0) };
    }
}

struct ListenerWindow {
    hwnd: HWND,
    instance: *mut c_void,
    class_name: Vec<u16>,
}

impl ListenerWindow {
    fn create() -> Result<Self, String> {
        let instance = unsafe { GetModuleHandleW(null()) };
        if instance.is_null() {
            return Err(win_error("无法获取应用实例"));
        }
        let class_name = wide("ClipstackClipboardListener");
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class_name.as_ptr(),
            ..Default::default()
        };
        if unsafe { RegisterClassW(&class) } == 0 {
            return Err(win_error("无法注册剪贴板监听窗口"));
        }
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class_name.as_ptr(),
                null(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                null_mut(),
                null_mut(),
                instance,
                null(),
            )
        };
        let window = Self {
            hwnd,
            instance,
            class_name,
        };
        if hwnd.is_null() {
            return Err(win_error("无法创建剪贴板监听窗口"));
        }
        if unsafe { AddClipboardFormatListener(hwnd) } == 0 {
            return Err(win_error("无法监听系统剪贴板"));
        }
        Ok(window)
    }
}

impl Drop for ListenerWindow {
    fn drop(&mut self) {
        unsafe {
            if !self.hwnd.is_null() {
                RemoveClipboardFormatListener(self.hwnd);
                DestroyWindow(self.hwnd);
            }
            UnregisterClassW(self.class_name.as_ptr(), self.instance);
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_QUERYENDSESSION => 1,
        WM_ENDSESSION => {
            if wparam != 0 {
                let callback = SESSION_END.with(|slot| slot.borrow().clone());
                if let Some(callback) = callback {
                    // A panic must never unwind through a Win32 window procedure.
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback()));
                }
            }
            0
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn run_listener(hwnd: HWND, on_change: impl Fn(Content), on_paste: impl Fn(u32, bool)) {
    let formats = RegisteredFormats::register();
    let mut last_sequence = None;
    let mut read_retries = 0;
    let mut paste_hook: HHOOK = null_mut();
    let mut message = MSG::default();
    while unsafe { GetMessageW(&mut message, null_mut(), 0, 0) } > 0 {
        match (message.message, message.wParam) {
            (WM_WATCH_PASTE, 1) if paste_hook.is_null() => {
                paste_hook = unsafe {
                    SetWindowsHookExW(
                        WH_KEYBOARD_LL,
                        Some(paste_key_proc),
                        GetModuleHandleW(null()),
                        0,
                    )
                };
            }
            (WM_WATCH_PASTE, 0) if !paste_hook.is_null() => {
                unsafe { UnhookWindowsHookEx(paste_hook) };
                paste_hook = null_mut();
            }
            (WM_PASTE_CHORD, sequence) => on_paste(sequence as u32, message.lParam != 0),
            (WM_CLIPBOARDUPDATE, _) | (WM_TIMER, READ_TIMER) => {
                if message.message == WM_CLIPBOARDUPDATE {
                    read_retries = READ_RETRIES;
                }
                match read_snapshot(hwnd, &formats, last_sequence) {
                    Ok((sequence, payload)) => {
                        unsafe { KillTimer(hwnd, READ_TIMER) };
                        last_sequence = Some(sequence);
                        if let Some(content) = payload.and_then(Snapshot::into_content) {
                            on_change(content);
                        }
                    }
                    Err(()) if read_retries > 0 => {
                        read_retries -= 1;
                        unsafe { SetTimer(hwnd, READ_TIMER, 15, None) };
                    }
                    Err(()) => {
                        unsafe { KillTimer(hwnd, READ_TIMER) };
                    }
                }
            }
            _ => unsafe {
                DispatchMessageW(&message);
            },
        }
    }
    if !paste_hook.is_null() {
        unsafe { UnhookWindowsHookEx(paste_hook) };
    }
}

unsafe extern "system" fn paste_key_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    use crate::paste::{external_focus, key_down, INPUT_TAG};
    let forwarded = unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) };
    if forwarded == 0 && code >= 0 && wparam == WM_KEYDOWN as usize {
        let input = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        let paste =
            (input.vkCode == u32::from(b'V') && key_down(VK_CONTROL) && !key_down(VK_SHIFT))
                || (input.vkCode == u32::from(VK_INSERT)
                    && key_down(VK_SHIFT)
                    && !key_down(VK_CONTROL));
        if input.dwExtraInfo != INPUT_TAG
            && paste
            && !key_down(VK_MENU)
            && !key_down(VK_LWIN)
            && !key_down(VK_RWIN)
        {
            let hwnd = LISTENER.load(Ordering::Acquire);
            unsafe {
                PostMessageW(
                    hwnd,
                    WM_PASTE_CHORD,
                    GetClipboardSequenceNumber() as usize,
                    isize::from(external_focus()),
                );
            }
        }
    }
    forwarded
}

struct OpenGuard;

impl OpenGuard {
    fn open(hwnd: HWND) -> Option<Self> {
        (unsafe { OpenClipboard(hwnd) } != 0).then(|| Self)
    }
}

impl Drop for OpenGuard {
    fn drop(&mut self) {
        unsafe { CloseClipboard() };
    }
}

enum Snapshot {
    Ready(Content),
    Png(Vec<u8>, Option<AccompanyingText>),
    Dib(NativeBitmap, Option<AccompanyingText>),
}

impl Snapshot {
    fn into_content(self) -> Option<Content> {
        match self {
            Self::Ready(content) => Some(content),
            Self::Png(png, rich) => {
                if let Ok(decoder) = PngDecoder::new(Cursor::new(&png)) {
                    if decoder.total_bytes() <= Limits::default().max_alloc.unwrap_or(u64::MAX) {
                        let (width, height) = decoder.dimensions();
                        return Some(Content::Image {
                            png,
                            width,
                            height,
                            rich,
                            bitmap: None,
                        });
                    }
                }
                rich.map(AccompanyingText::into_content)
            }
            Self::Dib(bitmap, rich) => {
                let mut bytes = bitmap.bytes.clone();
                normalize_dib_alpha(&mut bytes);
                if let Some(rgba) = decode_dib(bytes) {
                    let (width, height) = rgba.dimensions();
                    let mut png = Vec::new();
                    if PngEncoder::new(&mut png)
                        .write_image(rgba.as_raw(), width, height, ExtendedColorType::Rgba8)
                        .is_ok()
                    {
                        return Some(Content::Image {
                            png,
                            width,
                            height,
                            rich,
                            bitmap: Some(bitmap),
                        });
                    }
                }
                rich.map(AccompanyingText::into_content)
            }
        }
    }
}

fn read_snapshot(
    hwnd: HWND,
    formats: &RegisteredFormats,
    last_sequence: Option<u32>,
) -> Result<(u32, Option<Snapshot>), ()> {
    let clipboard = OpenGuard::open(hwnd).ok_or(())?;
    let sequence = unsafe { GetClipboardSequenceNumber() };
    if unsafe { GetClipboardOwner() } == hwnd {
        return Ok((sequence, None));
    }
    // A delayed format may advance the sequence only when GetClipboardData renders it.
    let rich = read_text_formats(&clipboard, formats);
    let data = if let Some(paths) = read_files() {
        Some(Snapshot::Ready(Content::Files { paths, rich }))
    } else if let Some(png) =
        read_bytes(&clipboard, formats.png).filter(|bytes| bytes.starts_with(b"\x89PNG\r\n\x1a\n"))
    {
        Some(Snapshot::Png(png, rich))
    } else if let Some(bitmap) = read_native_bitmap(&clipboard) {
        Some(Snapshot::Dib(bitmap, rich))
    } else {
        rich.map(|data| Snapshot::Ready(data.into_content()))
    };
    let rendered_sequence = unsafe { GetClipboardSequenceNumber() };
    if last_sequence == Some(rendered_sequence) {
        Ok((rendered_sequence, None))
    } else {
        Ok((rendered_sequence, data))
    }
}

fn read_native_bitmap(clipboard: &OpenGuard) -> Option<NativeBitmap> {
    for (format, version5) in [(CF_DIBV5, true), (CF_DIB, false)] {
        if let Some(bytes) = read_bytes(clipboard, u32::from(format)) {
            return Some(NativeBitmap { version5, bytes });
        }
    }
    None
}

fn read_text_formats(
    clipboard: &OpenGuard,
    formats: &RegisteredFormats,
) -> Option<AccompanyingText> {
    const MAX_RICH_FORMAT_SIZE: usize = 32 * 1024 * 1024;
    let unicode = |format| {
        read_bytes(clipboard, format)
            .map(|bytes| {
                let units: Vec<u16> = bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .copied()
                    .map(u16::from_le_bytes)
                    .take_while(|unit| *unit != 0)
                    .collect();
                String::from_utf16_lossy(&units)
            })
            .filter(|text| !text.is_empty())
    };
    let text = unicode(u32::from(CF_UNICODETEXT)).unwrap_or_default();
    let url = unicode(formats.url);
    let html = read_bytes_limited(clipboard, formats.html, MAX_RICH_FORMAT_SIZE)
        .map(|mut bytes| {
            if let Some(terminator) = bytes.iter().position(|byte| *byte == 0) {
                bytes.truncate(terminator + 1);
            }
            bytes
        })
        .filter(|bytes| bytes.first() != Some(&0));
    // RTF can contain a binary section with embedded NUL bytes.
    let rtf = read_bytes_limited(clipboard, formats.rtf, MAX_RICH_FORMAT_SIZE);
    (!text.is_empty() || html.is_some() || rtf.is_some() || url.is_some()).then_some(
        AccompanyingText {
            text,
            html,
            rtf,
            url,
        },
    )
}

fn read_bytes(clipboard: &OpenGuard, format: u32) -> Option<Vec<u8>> {
    read_bytes_limited(clipboard, format, isize::MAX as usize)
}

fn read_bytes_limited(_clipboard: &OpenGuard, format: u32, limit: usize) -> Option<Vec<u8>> {
    if format == 0 || unsafe { IsClipboardFormatAvailable(format) } == 0 {
        return None;
    }
    let handle = unsafe { GetClipboardData(format) };
    if handle.is_null() {
        return None;
    }
    let length = unsafe { GlobalSize(handle) };
    if length == 0 || length > limit {
        return None;
    }
    let data = unsafe { GlobalLock(handle) }.cast::<u8>();
    if data.is_null() {
        return None;
    }
    let bytes = unsafe { slice::from_raw_parts(data, length) }.to_vec();
    unsafe { GlobalUnlock(handle) };
    Some(bytes)
}

fn read_files() -> Option<Vec<String>> {
    let format = u32::from(CF_HDROP);
    if unsafe { IsClipboardFormatAvailable(format) } == 0 {
        return None;
    }
    let drop = unsafe { GetClipboardData(format) };
    if drop.is_null() {
        return None;
    }
    let count = unsafe { DragQueryFileW(drop, u32::MAX, null_mut(), 0) };
    let paths: Vec<String> = (0..count)
        .filter_map(|index| {
            let size = unsafe { DragQueryFileW(drop, index, null_mut(), 0) }.checked_add(1)?;
            let mut wide = vec![0; size as usize];
            let copied = unsafe { DragQueryFileW(drop, index, wide.as_mut_ptr(), size) };
            (copied > 0 && copied < size)
                .then(|| String::from_utf16_lossy(&wide[..copied as usize]))
        })
        .collect();
    (!paths.is_empty()).then_some(paths)
}

// Chromium can mark a 32-bit DIBV5 as BI_RGB while providing a usable alpha mask.
fn normalize_dib_alpha(bytes: &mut [u8]) {
    let Some(header_bytes) = bytes.get_mut(..size_of::<BITMAPV5HEADER>()) else {
        return;
    };
    let header = header_bytes.as_mut_ptr().cast::<BITMAPV5HEADER>();
    let mut value = unsafe { header.read_unaligned() };
    if value.bV5Size >= size_of::<BITMAPV5HEADER>() as u32
        && value.bV5BitCount == 32
        && value.bV5Compression == BI_RGB
        && value.bV5AlphaMask == 0xff00_0000
    {
        value.bV5Compression = BI_BITFIELDS;
        if value.bV5RedMask == 0 && value.bV5GreenMask == 0 && value.bV5BlueMask == 0 {
            value.bV5RedMask = 0x00ff_0000;
            value.bV5GreenMask = 0x0000_ff00;
            value.bV5BlueMask = 0x0000_00ff;
        }
        unsafe { header.write_unaligned(value) };
    }
}

fn decode_dib(bytes: Vec<u8>) -> Option<image::RgbaImage> {
    let read_dword = |start| {
        let field: [u8; 4] = bytes.get(start..start + 4)?.try_into().ok()?;
        Some(u32::from_le_bytes(field))
    };
    let header_size = read_dword(0)?;
    if matches!(header_size, 108 | 124) && read_dword(16)? == BI_BITFIELDS {
        // V4/V5 embed their channel masks inside the DIB header. A BMP file header
        // supplies an explicit pixel offset so the decoder does not skip extra masks.
        let colors_used = read_dword(32)?;
        let pixel_offset = 14_u32
            .checked_add(header_size)?
            .checked_add(colors_used.checked_mul(4)?)?;
        let file_size = u32::try_from(bytes.len()).ok()?.checked_add(14)?;
        let mut bmp = Vec::with_capacity(file_size as usize);
        bmp.extend_from_slice(b"BM");
        bmp.extend_from_slice(&file_size.to_le_bytes());
        bmp.extend_from_slice(&[0; 4]);
        bmp.extend_from_slice(&pixel_offset.to_le_bytes());
        bmp.extend(bytes);
        let decoder = BmpDecoder::new(Cursor::new(bmp)).ok()?;
        decode_image(decoder)
    } else {
        let decoder = BmpDecoder::new_without_file_header(Cursor::new(bytes)).ok()?;
        decode_image(decoder)
    }
}

fn decode_image(mut decoder: impl ImageDecoder) -> Option<image::RgbaImage> {
    let limits = Limits::default();
    if decoder.total_bytes() > limits.max_alloc.unwrap_or(u64::MAX) {
        return None;
    }
    decoder.set_limits(limits).ok()?;
    Some(DynamicImage::from_decoder(decoder).ok()?.into_rgba8())
}

pub fn sequence() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

pub fn write(content: &Content) -> Result<u32, String> {
    let hwnd = LISTENER.load(Ordering::Acquire);
    if hwnd.is_null() {
        return Err("剪贴板监听尚未启动".into());
    }
    // Prepare all formats before opening or clearing the system clipboard.
    let drop_effect_format = match content {
        Content::Files { .. } => register_format("Preferred DropEffect"),
        _ => 0,
    };
    let formats = write_formats(content, drop_effect_format)?;
    let mut clipboard = OpenGuard::open(hwnd);
    for _ in 0..READ_RETRIES {
        if clipboard.is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(5));
        clipboard = OpenGuard::open(hwnd);
    }
    let clipboard = clipboard.ok_or_else(|| "系统剪贴板正忙，请重试".to_string())?;
    if unsafe { EmptyClipboard() } == 0 {
        return Err(win_error("无法清空系统剪贴板以写入内容"));
    }
    let mut failed = false;
    for (format, data) in formats {
        let success = data.publish(format);
        failed |= !success && format != drop_effect_format;
    }
    if failed {
        return Err("无法写入一个或多个剪贴板格式，请重试".into());
    }
    // Windows can synthesize text formats when the clipboard is closed.
    // The paste guard needs the sequence of the committed clipboard contents.
    drop(clipboard);
    let committed = sequence();
    if unsafe { GetClipboardOwner() } != hwnd {
        return Err("系统剪贴板内容已被其他程序更改，请重试".into());
    }
    Ok(committed)
}

fn write_formats(
    content: &Content,
    drop_effect_format: u32,
) -> Result<Vec<(u32, GlobalData)>, String> {
    let mut formats = Vec::new();
    match content {
        Content::Text { text } => {
            add_unicode(&mut formats, u32::from(CF_UNICODETEXT), text)?;
        }
        Content::Link { text } => {
            add_text_formats(&mut formats, text, None, None, Some(text))?;
        }
        Content::RichText {
            text,
            html,
            rtf,
            url,
        } => {
            add_text_formats(
                &mut formats,
                text,
                html.as_deref(),
                rtf.as_deref(),
                url.as_deref(),
            )?;
        }
        Content::Files { paths, rich } => {
            if paths.is_empty()
                || paths
                    .iter()
                    .any(|path| path.is_empty() || path.contains('\0'))
            {
                return Err("文件列表没有有效路径".into());
            }
            let header = DROPFILES {
                pFiles: size_of::<DROPFILES>() as u32,
                fWide: 1,
                ..Default::default()
            };
            let mut bytes = bytes_of(&header).to_vec();
            bytes.extend(
                paths
                    .iter()
                    .flat_map(|path| wide(path))
                    .chain([0])
                    .flat_map(u16::to_le_bytes),
            );
            formats.push((u32::from(CF_HDROP), GlobalData::new(&bytes)?));
            if drop_effect_format != 0 {
                formats.push((
                    drop_effect_format,
                    GlobalData::new(&DROPEFFECT_COPY.to_le_bytes())?,
                ));
            }
            add_accompanying_text(&mut formats, rich.as_ref())?;
        }
        Content::Image {
            png, rich, bitmap, ..
        } => {
            if let Some(bitmap) = bitmap {
                let format = if bitmap.version5 { CF_DIBV5 } else { CF_DIB };
                formats.push((u32::from(format), GlobalData::new(&bitmap.bytes)?));
                formats.push((required_format("PNG")?, GlobalData::new(png)?));
            } else {
                let decoder =
                    PngDecoder::new(Cursor::new(png)).map_err(|_| "图像内容损坏".to_string())?;
                let rgba = decode_image(decoder).ok_or_else(|| "无法解码图像内容".to_string())?;
                let dib = encode_dib(rgba)?;
                formats.push((required_format("PNG")?, GlobalData::new(png)?));
                formats.push((u32::from(CF_DIBV5), GlobalData::new(&dib)?));
            }
            add_accompanying_text(&mut formats, rich.as_ref())?;
        }
    }
    if formats.is_empty() {
        Err("没有可以写入的剪贴板内容".into())
    } else {
        Ok(formats)
    }
}

fn add_accompanying_text(
    formats: &mut Vec<(u32, GlobalData)>,
    rich: Option<&AccompanyingText>,
) -> Result<(), String> {
    if let Some(data) = rich {
        add_text_formats(
            formats,
            &data.text,
            data.html.as_deref(),
            data.rtf.as_deref(),
            data.url.as_deref(),
        )?;
    }
    Ok(())
}

fn add_text_formats(
    formats: &mut Vec<(u32, GlobalData)>,
    text: &str,
    html: Option<&[u8]>,
    rtf: Option<&[u8]>,
    url: Option<&str>,
) -> Result<(), String> {
    for (name, bytes) in [("Rich Text Format", rtf), ("HTML Format", html)] {
        if let Some(bytes) = bytes.filter(|bytes| !bytes.is_empty()) {
            formats.push((required_format(name)?, GlobalData::new(bytes)?));
        }
    }
    if let Some(url) = url.filter(|value| !value.is_empty()) {
        add_unicode(formats, required_format("UniformResourceLocatorW")?, url)?;
    }
    if !text.is_empty() {
        add_unicode(formats, u32::from(CF_UNICODETEXT), text)?;
    }
    Ok(())
}

fn add_unicode(
    formats: &mut Vec<(u32, GlobalData)>,
    format: u32,
    text: &str,
) -> Result<(), String> {
    if text.contains('\0') {
        return Err("文本含有剪贴板无法保存的空字符".into());
    }
    let bytes = if format == u32::from(CF_UNICODETEXT) {
        clipboard_text_bytes(text)
    } else {
        wide(text).into_iter().flat_map(u16::to_le_bytes).collect()
    };
    formats.push((format, GlobalData::new(&bytes)?));
    Ok(())
}

fn clipboard_text_bytes(text: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut units = text.encode_utf16().peekable();
    while let Some(unit) = units.next() {
        if unit == u16::from(b'\r') || unit == u16::from(b'\n') {
            if unit == u16::from(b'\r') && units.peek() == Some(&u16::from(b'\n')) {
                units.next();
            }
            bytes.extend_from_slice(&[b'\r', 0, b'\n', 0]);
        } else {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
    }
    bytes.extend_from_slice(&[0, 0]);
    bytes
}

fn required_format(name: &str) -> Result<u32, String> {
    let format = register_format(name);
    if format == 0 {
        Err(win_error(&format!("无法注册剪贴板格式 {name}")))
    } else {
        Ok(format)
    }
}

pub fn html_fragment(bytes: &[u8]) -> Option<&str> {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    let html = std::str::from_utf8(&bytes[..end]).ok()?;
    let header = html.split_once('<').map_or(html, |(head, _)| head);
    let offset = |field: &str| {
        header.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case(field)
                .then_some(value.trim().parse::<usize>().ok())
                .flatten()
        })
    };
    if let (Some(start), Some(end)) = (offset("StartFragment"), offset("EndFragment")) {
        if let Some(fragment) = html.get(start..end) {
            return Some(fragment);
        }
    }
    if let Some((_, remainder)) = html.split_once("<!--StartFragment-->") {
        if let Some((fragment, _)) = remainder.split_once("<!--EndFragment-->") {
            return Some(fragment);
        }
    }
    if let (Some(start), Some(end)) = (offset("StartHTML"), offset("EndHTML")) {
        if let Some(fragment) = html.get(start..end) {
            return Some(fragment);
        }
    }
    html.find('<').map(|start| &html[start..])
}

fn encode_dib(rgba: image::RgbaImage) -> Result<Vec<u8>, String> {
    let (width, height) = rgba.dimensions();
    let invalid_size = |_| "图像尺寸超出剪贴板支持范围".to_string();
    let header = BITMAPV5HEADER {
        bV5Size: size_of::<BITMAPV5HEADER>() as u32,
        bV5Width: i32::try_from(width).map_err(invalid_size)?,
        bV5Height: i32::try_from(height).map_err(invalid_size)?,
        bV5Planes: 1,
        bV5BitCount: 32,
        bV5Compression: BI_BITFIELDS,
        bV5SizeImage: u32::try_from(rgba.as_raw().len()).map_err(invalid_size)?,
        bV5RedMask: 0x00ff_0000,
        bV5GreenMask: 0x0000_ff00,
        bV5BlueMask: 0x0000_00ff,
        bV5AlphaMask: 0xff00_0000,
        bV5CSType: 0x7352_4742, // LCS_sRGB
        bV5Intent: LCS_GM_IMAGES as u32,
        ..Default::default()
    };
    let mut bytes = Vec::with_capacity(size_of::<BITMAPV5HEADER>() + rgba.as_raw().len());
    bytes.extend_from_slice(bytes_of(&header));
    // Positive-height, bottom-up DIBs also paste into Word and other older editors.
    for row in rgba.as_raw().chunks_exact(width as usize * 4).rev() {
        for &[red, green, blue, alpha] in row.as_chunks::<4>().0 {
            bytes.extend_from_slice(&[blue, green, red, alpha]);
        }
    }
    Ok(bytes)
}

struct GlobalData(HGLOBAL);

impl GlobalData {
    fn new(bytes: &[u8]) -> Result<Self, String> {
        let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) };
        if handle.is_null() {
            return Err(win_error("无法为剪贴板分配内存"));
        }
        let data = Self(handle);
        let buffer = unsafe { GlobalLock(handle) }.cast::<u8>();
        if buffer.is_null() {
            return Err(win_error("无法访问剪贴板内存"));
        }
        unsafe {
            buffer.copy_from_nonoverlapping(bytes.as_ptr(), bytes.len());
            GlobalUnlock(handle);
        }
        Ok(data)
    }

    fn publish(mut self, format: u32) -> bool {
        let success = !unsafe { SetClipboardData(format, self.0) }.is_null();
        if success {
            self.0 = null_mut(); // SetClipboardData takes ownership on success.
        }
        success
    }
}

impl Drop for GlobalData {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { GlobalFree(self.0) };
        }
    }
}

// These calls only pass fully initialized Win32 headers made of fixed-size integers.
fn bytes_of<T>(value: &T) -> &[u8] {
    unsafe { slice::from_raw_parts((value as *const T).cast::<u8>(), size_of::<T>()) }
}

fn register_format(name: &str) -> u32 {
    unsafe { RegisterClipboardFormatW(wide(name).as_ptr()) }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

fn win_error(context: &str) -> String {
    format!("{context}：{}", io::Error::last_os_error())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_text_preserves_unicode_and_uses_windows_line_endings() {
        let expected: Vec<_> = wide("剪栈📋\r\n第二行\r\nThird\tline\r\n第四行")
            .into_iter()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(
            clipboard_text_bytes("剪栈📋\n第二行\r\nThird\tline\r第四行"),
            expected
        );
    }

    #[test]
    fn html_fragment_uses_byte_offsets_and_supports_marker_fallback() {
        let document = "<html><body><strong>剪栈👋</strong></body></html>";
        let selected = "<strong>剪栈👋</strong>";
        let header = "Version:1.0\r\nStartFragment:0000000000\r\nEndFragment:0000000000\r\n";
        let start = header.len() + document.find(selected).unwrap();
        let end = start + selected.len();
        let clipboard = format!(
            "Version:1.0\r\nStartFragment:{start:010}\r\nEndFragment:{end:010}\r\n{document}\0"
        );
        assert_eq!(html_fragment(clipboard.as_bytes()), Some(selected));
        assert_eq!(
            html_fragment(b"Version:0.9\r\nStartFragment:-1\r\n<html><!--StartFragment--><b>x</b><!--EndFragment--></html>"),
            Some("<b>x</b>")
        );
    }

    #[test]
    fn rich_formats_retain_binary_bytes_and_are_listed_before_plain_text() {
        let content = Content::RichText {
            text: "剪栈".into(),
            html: Some(b"<b>clip</b>\0".to_vec()),
            rtf: Some(b"{\\rtf1{\\pict\\bin1 \0}clip}\0".to_vec()),
            url: Some("https://example.com".into()),
        };
        let serialized = serde_json::to_string(&content).unwrap();
        assert_eq!(
            serde_json::from_str::<Content>(&serialized).unwrap(),
            content
        );
        let formats = write_formats(&content, 0).unwrap();
        let expected = [
            (
                register_format("Rich Text Format"),
                b"{\\rtf1{\\pict\\bin1 \0}clip}\0".to_vec(),
            ),
            (register_format("HTML Format"), b"<b>clip</b>\0".to_vec()),
            (
                register_format("UniformResourceLocatorW"),
                wide("https://example.com")
                    .into_iter()
                    .flat_map(u16::to_le_bytes)
                    .collect(),
            ),
            (
                u32::from(CF_UNICODETEXT),
                wide("剪栈")
                    .into_iter()
                    .flat_map(u16::to_le_bytes)
                    .collect(),
            ),
        ];
        assert_eq!(formats.len(), expected.len());
        for ((format, memory), (expected_id, expected_bytes)) in formats.into_iter().zip(expected) {
            assert_eq!(format, expected_id);
            unsafe {
                let ptr = GlobalLock(memory.0).cast::<u8>();
                assert!(!ptr.is_null());
                assert_eq!(
                    slice::from_raw_parts(ptr, expected_bytes.len()),
                    expected_bytes
                );
                GlobalUnlock(memory.0);
            }
        }
    }

    #[test]
    fn old_entries_load_without_new_representation_fields() {
        let old = r#"{"kind":"files","paths":["C:/test.txt"]}"#;
        assert!(matches!(
            serde_json::from_str::<Content>(old).unwrap(),
            Content::Files { rich: None, .. }
        ));
        let old = r#"{"kind":"image","png":"","width":1,"height":1}"#;
        assert!(matches!(
            serde_json::from_str::<Content>(old).unwrap(),
            Content::Image {
                rich: None,
                bitmap: None,
                ..
            }
        ));
    }
}
