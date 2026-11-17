#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod clipboard;
mod context_menu;
mod model;
mod paste;
mod preview;
mod shell;
mod storage;
mod windows_integration;

use std::{path::PathBuf, sync::Mutex};

use base64::{engine::general_purpose::STANDARD, Engine};
use clipboard::Content;
use model::{AppView, Model, PasteMode, SettingsPatch};
use shell::AppModel;
use tauri::{AppHandle, Manager, State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

#[tauri::command]
fn get_state(model: State<AppModel>, query: Option<String>) -> AppView {
    let mut model = model.lock().unwrap();
    model.settings.autostart = shell::autostart_enabled();
    model.view(query.as_deref().unwrap_or_default())
}

#[tauri::command]
async fn read_image(id: String, model: State<'_, AppModel>) -> Result<String, String> {
    let content = model.lock().unwrap().get(&id)?.content.clone();
    match content {
        Content::Image { png, .. } => Ok(format!("data:image/png;base64,{}", STANDARD.encode(png))),
        Content::Files { paths, .. } => {
            tauri::async_runtime::spawn_blocking(move || preview::file_thumbnail(&paths))
                .await
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "无法读取此图片。".into())
        }
        _ => Err("这条记录不是图片。".into()),
    }
}

#[tauri::command]
fn read_text(id: String, model: State<AppModel>) -> Result<String, String> {
    let model = model.lock().unwrap();
    match &model.get(&id)?.content {
        Content::Text { text } | Content::Link { text } | Content::RichText { text, .. } => {
            Ok(text.clone())
        }
        _ => Err("这条记录不是文本。".into()),
    }
}

#[tauri::command]
fn open_preview(app: AppHandle, id: String, model: State<AppModel>) -> Result<(), String> {
    {
        let mut state = model.lock().unwrap();
        if id != preview::MANUAL_ID {
            state.get(&id)?;
        }
        state.preview_id = Some(id.clone());
    }
    shell::show_preview(&app, &id)
}

#[tauri::command]
fn get_preview(model: State<AppModel>) -> Option<String> {
    model.lock().unwrap().preview_id.clone()
}

#[tauri::command]
async fn read_preview(
    id: String,
    index: Option<usize>,
    model: State<'_, AppModel>,
) -> Result<preview::Preview, String> {
    if id == preview::MANUAL_ID {
        return Ok(preview::manual());
    }
    let content = model.lock().unwrap().get(&id)?.content.clone();
    tauri::async_runtime::spawn_blocking(move || {
        preview::preview(&content, index.unwrap_or_default())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
fn open_release_page() -> Result<(), String> {
    use std::ptr::{null, null_mut};
    use windows::core::w;
    use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};

    let status = unsafe {
        ShellExecuteW(
            null_mut(),
            w!("open").as_ptr(),
            w!("https://github.com/marumaruYGR/clipstack/releases").as_ptr(),
            null(),
            null(),
            SW_SHOWNORMAL,
        )
    };
    if status as isize <= 32 {
        return Err("无法打开剪栈发布页。".into());
    }
    Ok(())
}

#[tauri::command]
async fn locate_file(id: String, index: usize, model: State<'_, AppModel>) -> Result<(), String> {
    let path = {
        let state = model.lock().unwrap();
        match &state.get(&id)?.content {
            Content::Files { paths, .. } => paths.get(index).map(PathBuf::from),
            _ => None,
        }
        .ok_or_else(|| "这条记录没有对应的文件。".to_string())?
    };
    tauri::async_runtime::spawn_blocking(move || windows_integration::locate(&path))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
fn copy_item(
    app: AppHandle,
    window: WebviewWindow,
    id: String,
    model: State<AppModel>,
) -> Result<(), String> {
    let (content, mode, pinned) = {
        let state = model.lock().unwrap();
        (
            state.get(&id)?.content.clone(),
            state.settings.paste_mode,
            state.pinned,
        )
    };
    let sequence = clipboard::write(&content)?;
    let close_quick = window.label() == "quick" && !pinned;
    {
        let mut state = model.lock().unwrap();
        state.pending_manual_paste =
            (mode == PasteMode::Manual && state.settings.notifications).then_some(sequence);
    }

    if mode == PasteMode::Manual {
        if close_quick {
            let _ = window.hide();
        }
        shell::show_notice(&app, shell::PICK_NOTICE);
        show_first_paste_tip(&app, mode);
        return Ok(());
    }

    let Some(target) = paste::last_target() else {
        if close_quick {
            let _ = window.hide();
        }
        shell::show_notice(&app, shell::PASTE_FALLBACK_NOTICE);
        show_first_paste_tip(&app, mode);
        return Ok(());
    };
    tauri::async_runtime::spawn_blocking(move || {
        let sent = paste::send_paste(&app, target, sequence);
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            if clipboard::sequence() != sequence {
                return;
            }
            if close_quick && !handle.state::<AppModel>().lock().unwrap().pinned {
                let _ = window.hide();
            }
            let (notice, position) = if sent {
                (shell::SENT_NOTICE, target.center())
            } else {
                (shell::PASTE_FALLBACK_NOTICE, None)
            };
            shell::show_notice_at(&handle, notice, position);
            show_first_paste_tip(&handle, mode);
        });
    });
    Ok(())
}

fn show_first_paste_tip(app: &AppHandle, mode: PasteMode) {
    let state = app.state::<AppModel>();
    let mut model = state.lock().unwrap();
    let seen = match mode {
        PasteMode::Direct => model.settings.seen_direct_paste,
        PasteMode::Manual => model.settings.seen_manual_paste,
    };
    if seen {
        return;
    }
    if let Err(error) = windows_integration::show_paste_mode_tip(mode == PasteMode::Direct) {
        eprintln!("无法发送粘贴方式系统通知：{error}");
        return;
    }
    match mode {
        PasteMode::Direct => model.settings.seen_direct_paste = true,
        PasteMode::Manual => model.settings.seen_manual_paste = true,
    }
    if let Err(error) = model.storage.save_settings(&model.settings) {
        eprintln!("无法保存粘贴方式提示状态：{error}");
    }
}

#[tauri::command]
fn add_content(app: AppHandle, text: String, model: State<AppModel>) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("请输入要添加的内容。".into());
    }
    let content = Content::Text { text };
    clipboard::write(&content)?;
    let mut model = model.lock().unwrap();
    model.pending_manual_paste = None;
    let result = model.add(content);
    drop(model);
    shell::sync_paste_watch(&app);
    shell::state_changed(&app);
    result
}

#[tauri::command]
fn delete_item(
    app: AppHandle,
    id: String,
    from_favorites: bool,
    model: State<AppModel>,
) -> Result<(), String> {
    model.lock().unwrap().delete(&id, from_favorites)?;
    shell::state_changed(&app);
    Ok(())
}

#[tauri::command]
fn delete_items(app: AppHandle, ids: Vec<String>, model: State<AppModel>) -> Result<(), String> {
    model
        .lock()
        .unwrap()
        .delete_history(ids.iter().map(String::as_str))?;
    shell::state_changed(&app);
    Ok(())
}

#[tauri::command]
fn clear_history(app: AppHandle, model: State<AppModel>) -> Result<(), String> {
    model.lock().unwrap().clear()?;
    shell::state_changed(&app);
    Ok(())
}

#[tauri::command]
fn toggle_favorite(app: AppHandle, id: String, model: State<AppModel>) -> Result<(), String> {
    model.lock().unwrap().toggle_favorite(&id)?;
    shell::state_changed(&app);
    Ok(())
}

#[tauri::command]
fn open_main(app: AppHandle) -> Result<(), String> {
    shell::show_main(&app)
}

#[tauri::command]
fn take_search_request(request: State<shell::SearchRequest>) -> bool {
    request.take()
}

#[tauri::command]
fn toggle_quick(app: AppHandle) -> Result<(), String> {
    shell::toggle_quick(&app, None)
}

#[tauri::command]
fn set_pinned(app: AppHandle, pinned: bool, model: State<AppModel>) {
    model.lock().unwrap().pinned = pinned;
    shell::state_changed(&app);
}

#[tauri::command]
async fn choose_directory(app: AppHandle, window: WebviewWindow) -> Option<String> {
    let directory = app
        .state::<AppModel>()
        .lock()
        .unwrap()
        .settings
        .favorites_directory
        .clone();
    let (tx, rx) = tauri::async_runtime::channel(1);
    app.dialog()
        .file()
        .set_title("选择收藏夹存放位置")
        .set_directory(directory)
        .set_parent(&window)
        .pick_folder(move |path| {
            let _ = tx.try_send(path);
        });
    let mut rx = rx;
    rx.recv()
        .await
        .flatten()
        .and_then(|path| path.into_path().ok())
        .map(|path| path.to_string_lossy().into_owned())
}

#[tauri::command]
fn move_favorites(
    app: AppHandle,
    directory: PathBuf,
    model: State<AppModel>,
) -> Result<(), String> {
    model.lock().unwrap().move_favorites(directory)?;
    shell::state_changed(&app);
    Ok(())
}

#[tauri::command]
fn restart_app(app: AppHandle) {
    app.request_restart();
}

#[tauri::command]
fn window_action(
    window: WebviewWindow,
    action: String,
    origin: Option<shell::PointerOrigin>,
) -> Result<(), String> {
    shell::window_action(&window, &action, origin)
}

fn parse_shortcut(value: &str) -> Result<Shortcut, String> {
    let shortcut = value
        .parse::<Shortcut>()
        .map_err(|_| "无法识别此快捷键，请按下一个组合键。".to_string())?;
    use tauri_plugin_global_shortcut::{Code, Modifiers};
    if shortcut.mods.is_empty() {
        return Err("快捷键至少需要一个修饰键。".into());
    }
    if shortcut.mods == Modifiers::CONTROL
        && matches!(
            shortcut.key,
            Code::KeyC
                | Code::KeyV
                | Code::KeyX
                | Code::KeyA
                | Code::KeyZ
                | Code::KeyY
                | Code::KeyS
        )
    {
        return Err(format!(
            "{value} 已用于常见系统或应用操作，请选择其他组合键。"
        ));
    }
    Ok(shortcut)
}

fn shortcut_conflict(value: &str) -> String {
    format!("{value} 无法注册，可能已被 Windows 或其他应用占用。")
}

#[tauri::command]
fn update_settings(
    app: AppHandle,
    patch: SettingsPatch,
    state: State<AppModel>,
) -> Result<(), String> {
    let (old, registered) = {
        let model = state.lock().unwrap();
        (model.settings.clone(), model.registered)
    };
    let mut next = old.clone();
    if let Some(value) = patch.material {
        next.material = value;
    }
    if let Some(value) = patch.theme {
        next.theme = value;
    }
    if let Some(value) = patch.autostart {
        next.autostart = value;
    }
    if let Some(value) = patch.notifications {
        next.notifications = value;
    }
    if let Some(value) = patch.paste_mode {
        next.paste_mode = value;
    }
    let update_shortcut = patch.shortcut.is_some();
    if let Some(value) = patch.shortcut {
        next.shortcut = value;
    }
    let desired = if update_shortcut {
        next.shortcut.as_deref().map(parse_shortcut).transpose()?
    } else {
        registered
    };
    let shortcuts = app.global_shortcut();
    let changed = registered != desired;
    if changed {
        if let Some(key) = desired {
            shortcuts
                .register(key)
                .map_err(|_| shortcut_conflict(next.shortcut.as_deref().unwrap_or_default()))?;
        }
        if let Some(key) = registered {
            if let Err(error) = shortcuts.unregister(key) {
                if let Some(key) = desired {
                    let _ = shortcuts.unregister(key);
                }
                return Err(format!("无法释放原快捷键：{error}"));
            }
        }
    }
    let update_autostart = patch.autostart.is_some();
    let result = (|| {
        if update_autostart {
            shell::set_autostart(next.autostart)?;
        }
        state.lock().unwrap().storage.save_settings(&next)
    })();
    if let Err(error) = result {
        if update_autostart {
            let _ = shell::set_autostart(old.autostart);
        }
        if changed {
            if let Some(key) = desired {
                let _ = shortcuts.unregister(key);
            }
            if let Some(key) = registered {
                let _ = shortcuts.register(key);
            }
        }
        return Err(error);
    }
    {
        let mut model = state.lock().unwrap();
        model.settings = next.clone();
        if old.paste_mode != next.paste_mode || !next.notifications {
            model.pending_manual_paste = None;
        }
        model.registered = desired;
        if update_shortcut {
            model.shortcut_error = None;
        }
    }
    if !next.notifications {
        shell::hide_notice(&app, None);
    } else if old.paste_mode != next.paste_mode {
        shell::sync_paste_watch(&app);
    }
    if old.theme != next.theme || old.material != next.material {
        shell::apply_appearance(&app, &next);
    }
    shell::state_changed(&app);
    Ok(())
}

fn start(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    app.manage(Mutex::new(Model::open()?));
    shell::setup(app)?;
    if let Err(error) = windows_integration::register_search_task() {
        eprintln!("无法注册任务栏检索：{error}");
    }
    for window in app.webview_windows().values() {
        context_menu::install(window)?;
    }
    let shortcut = app
        .state::<AppModel>()
        .lock()
        .unwrap()
        .settings
        .shortcut
        .clone();
    if let Some(value) = shortcut {
        let registration = parse_shortcut(&value).and_then(|key| {
            app.global_shortcut()
                .register(key)
                .map(|_| key)
                .map_err(|_| shortcut_conflict(&value))
        });
        let state = app.state::<AppModel>();
        let mut model = state.lock().unwrap();
        match registration {
            Ok(key) => model.registered = Some(key),
            Err(error) => model.shortcut_error = Some(error),
        }
    }

    let changed = app.clone();
    let pasted = app.clone();
    let session = app.clone();
    clipboard::start(
        move |content| {
            let handle = changed.clone();
            let added = {
                let state = changed.state::<AppModel>();
                let mut model = state.lock().unwrap();
                if model.pending_manual_paste != Some(clipboard::sequence()) {
                    model.pending_manual_paste = None;
                }
                model.add(content).is_ok()
            };
            if added {
                let _ = changed
                    .run_on_main_thread(move || shell::show_notice(&handle, shell::COPY_NOTICE));
            }
            shell::state_changed(&changed);
        },
        move |sequence, external_focus| {
            let handle = pasted.clone();
            let _ = pasted.run_on_main_thread(move || {
                let (show_confirmation, dismiss) = {
                    let state = handle.state::<AppModel>();
                    let mut model = state.lock().unwrap();
                    if model.pending_manual_paste != Some(clipboard::sequence()) {
                        model.pending_manual_paste = None;
                    }
                    let matches = external_focus && model.pending_manual_paste == Some(sequence);
                    if matches {
                        model.pending_manual_paste = None;
                    }
                    let dismiss = matches!(
                        model.notice,
                        Some(
                            shell::COPY_NOTICE | shell::PICK_NOTICE | shell::PASTE_FALLBACK_NOTICE
                        )
                    );
                    (matches, dismiss)
                };
                if show_confirmation {
                    shell::show_notice(&handle, shell::PASTED_NOTICE);
                } else if dismiss {
                    shell::hide_notice(&handle, None);
                } else {
                    shell::sync_paste_watch(&handle);
                }
            });
        },
        move || {
            let _ = session
                .state::<AppModel>()
                .lock()
                .unwrap()
                .storage
                .clear_session_history();
        },
    )?;

    if !std::env::args_os().any(|arg| arg == "--autostart") {
        if std::env::args_os().any(|arg| arg == "--search") {
            shell::show_search(app)?;
        } else {
            shell::show_main(app)?;
        }
    }
    Ok(())
}

fn main() {
    if let Some(launch) = configure_install_account() {
        if !launch {
            return;
        }
    }
    if std::env::args_os().any(|arg| arg == "--autostart") && !shell::autostart_enabled() {
        return;
    }
    windows_integration::set_app_id();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _| {
            if !args.iter().any(|arg| arg == "--autostart") {
                if args.iter().any(|arg| arg == "--search") {
                    let _ = shell::show_search(app);
                } else {
                    let _ = shell::show_main(app);
                }
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _, event| {
                    if event.state() == ShortcutState::Pressed {
                        let _ = shell::toggle_quick(app, None);
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            get_state,
            read_image,
            read_text,
            open_preview,
            get_preview,
            read_preview,
            open_release_page,
            locate_file,
            copy_item,
            add_content,
            delete_item,
            delete_items,
            clear_history,
            toggle_favorite,
            open_main,
            take_search_request,
            toggle_quick,
            set_pinned,
            choose_directory,
            move_favorites,
            restart_app,
            window_action,
            update_settings
        ])
        .setup(|app| start(app.handle()));
    if let Err(error) = app.run(tauri::generate_context!()) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR};
        let message: Vec<u16> = format!("剪栈启动失败。\n{error}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let title: Vec<u16> = "剪栈".encode_utf16().chain(Some(0)).collect();
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                message.as_ptr(),
                title.as_ptr(),
                MB_ICONERROR,
            );
        }
    }
}

// The elevated installer delegates current-user settings to Explorer's user token.
fn configure_install_account() -> Option<bool> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let flag = |index: usize| match args.get(index).and_then(|arg| arg.to_str()) {
        Some("0") => Some(false),
        Some("1") => Some(true),
        _ => None,
    };
    let (startup, start_menu, desktop, launch, register) =
        match args.first().and_then(|arg| arg.to_str()) {
            Some("--configure-install-user") => match (flag(1), flag(2), flag(3), flag(4)) {
                (Some(startup), Some(start_menu), Some(desktop), Some(launch)) => {
                    (Some(startup), start_menu, desktop, launch, true)
                }
                _ => return Some(false),
            },
            Some("--configure-install-all-user") => match (flag(1), flag(2)) {
                (Some(startup), Some(launch)) => (Some(startup), false, false, launch, false),
                _ => return Some(false),
            },
            Some("--remove-install-user") => (None, false, false, false, false),
            _ => return None,
        };
    if let Some(account) = args.iter().position(|arg| arg == "--installer-account") {
        let verified = args
            .get(account + 1)
            .and_then(|arg| arg.to_str())
            .is_some_and(windows_integration::signed_in_account_matches);
        if !verified {
            return Some(false);
        }
    }
    let result_token = args.windows(2).find_map(|pair| {
        if pair[0] != "--installer-result" {
            return None;
        }
        pair[1].to_str().filter(|token| {
            !token.is_empty()
                && token.len() <= 64
                && token.bytes().all(|ch| ch.is_ascii_hexdigit() || ch == b'-')
        })
    });
    let startup = match startup {
        Some(enabled) if register => shell::set_install_user_autostart(enabled),
        Some(enabled) => shell::set_autostart(enabled),
        None => shell::remove_user_autostart(),
    };
    let links = windows_integration::configure_user_shortcuts(start_menu, desktop);
    let registration = windows_integration::configure_user_uninstall(register);
    let result = startup
        .and(links.map_err(|error| error.to_string()))
        .and(registration.map_err(|error| error.to_string()));
    if let Some(token) = result_token {
        let reported = shell::report_installer_result(token, result.is_ok()).is_ok();
        return Some(reported && result.is_ok() && launch);
    }
    if let Err(error) = result {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONWARNING};
        let text: Vec<u16> = format!("剪栈已安装，但未能完成此账户的设置。\n{error}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let title: Vec<u16> = "剪栈".encode_utf16().chain(Some(0)).collect();
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                title.as_ptr(),
                MB_ICONWARNING,
            );
        }
    }
    Some(launch)
}
