use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};

const LATEST: &str = "https://api.github.com/repos/marumaruYGR/clipstack/releases/latest";
const MAX_DOWNLOAD: u64 = 512 * 1024 * 1024;
const DAY: u64 = 24 * 60 * 60;

#[derive(Clone, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    body: Option<String>,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    version: String,
    notes: String,
    size: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheck {
    current_version: String,
    update: Option<UpdateInfo>,
}

#[derive(Clone)]
struct Available {
    info: UpdateInfo,
    asset: Asset,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
struct CheckHistory {
    last_check: u64,
    notified: String,
}

struct UpdateState {
    history: CheckHistory,
    available: Option<Available>,
    installing: bool,
}

struct UpdateService {
    state: Mutex<UpdateState>,
    checking: Mutex<()>,
    directory: PathBuf,
}

pub fn display_version() -> String {
    env!("CARGO_PKG_VERSION").replace('+', "fix")
}

// Build metadata identifies the maintenance fix, which SemVer precedence ignores.
fn version(text: &str) -> Option<[u64; 4]> {
    let text = text.strip_prefix('v').unwrap_or(text);
    let (base, fix) = text
        .split_once("fix")
        .or_else(|| text.split_once('+'))
        .unwrap_or((text, "0"));
    let numbers = base
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    if numbers.len() != 3 || fix.is_empty() {
        return None;
    }
    Some([numbers[0], numbers[1], numbers[2], fix.parse().ok()?])
}

fn client(timeout: u64) -> Result<Client, String> {
    Client::builder()
        .user_agent(format!("Clipstack/{}", display_version()))
        .https_only(true)
        .redirect(reqwest::redirect::Policy::limited(8))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(timeout))
        .build()
        .map_err(|error| format!("无法建立更新连接：{error}"))
}

fn available(release: Release) -> Result<Option<Available>, String> {
    if release.draft || release.prerelease {
        return Ok(None);
    }
    let latest = version(&release.tag_name).ok_or("GitHub 版本号格式无法识别。")?;
    if latest <= version(env!("CARGO_PKG_VERSION")).ok_or("当前版本号无效。")? {
        return Ok(None);
    }
    let name = release
        .tag_name
        .strip_prefix('v')
        .unwrap_or(&release.tag_name)
        .to_string();
    let expected = format!("clipstack-{name}-windows-x64-setup.exe");
    let asset = release
        .assets
        .into_iter()
        .find(|asset| asset.name == expected)
        .ok_or("检测到新版，但 Windows x64 安装包尚未发布，请稍后重试。")?;
    let expected_url = format!(
        "https://github.com/marumaruYGR/clipstack/releases/download/{}/{}",
        release.tag_name, asset.name
    );
    if asset.browser_download_url != expected_url || asset.size == 0 || asset.size > MAX_DOWNLOAD {
        return Err("新版安装包地址或大小无效。".into());
    }
    let hash = asset
        .digest
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .ok_or("新版安装包缺少 SHA-256 校验信息，请稍后重试。")?;
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("新版安装包校验信息无效。".into());
    }
    Ok(Some(Available {
        info: UpdateInfo {
            version: name,
            notes: release
                .body
                .unwrap_or_else(|| "此版本未提供更新日志。".into()),
            size: asset.size,
        },
        asset,
    }))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn check(app: &AppHandle, automatic: bool) -> Result<UpdateCheck, String> {
    let service = app.state::<UpdateService>();
    let _checking = service.checking.lock().unwrap();
    if automatic {
        let state = service.state.lock().unwrap();
        if state.history.last_check != 0 && now().saturating_sub(state.history.last_check) < DAY {
            return Ok(UpdateCheck {
                current_version: display_version(),
                update: None,
            });
        }
    }
    let response = client(25)?
        .get(LATEST)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| format!("检查更新失败，请检查网络后重试。{error}"))?;
    let mut bytes = Vec::new();
    response
        .take(2 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("无法读取更新信息：{error}"))?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("GitHub 更新信息过大。".into());
    }
    let release = serde_json::from_slice(&bytes)
        .map_err(|error| format!("无法解析 GitHub 更新信息：{error}"))?;
    let next = available(release)?;
    let mut state = service.state.lock().unwrap();
    state.history.last_check = now();
    if automatic {
        if let Some(update) = &next {
            if state.history.notified != update.info.version {
                crate::windows_integration::show_update_available()
                    .map_err(|error| format!("无法发送更新通知：{error}"))?;
                state.history.notified.clone_from(&update.info.version);
            }
        }
    }
    let info = next.as_ref().map(|update| update.info.clone());
    state.available = next;
    let bytes = serde_json::to_vec(&state.history).map_err(|error| error.to_string())?;
    crate::storage::write_backup_file(&service.directory.join("update-state.json"), &bytes)?;
    Ok(UpdateCheck {
        current_version: display_version(),
        update: info,
    })
}

pub fn start(app: &AppHandle) -> Result<(), String> {
    let directory = app
        .path()
        .app_local_data_dir()
        .map_err(|error| error.to_string())?
        .join("updates");
    fs::create_dir_all(&directory).map_err(|error| format!("无法创建更新目录：{error}"))?;
    let history = fs::read(directory.join("update-state.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    app.manage(UpdateService {
        state: Mutex::new(UpdateState {
            history,
            available: None,
            installing: false,
        }),
        checking: Mutex::new(()),
        directory,
    });
    let handle = app.clone();
    std::thread::Builder::new()
        .name("clipstack-updates".into())
        .spawn(move || loop {
            if let Err(error) = check(&handle, true) {
                eprintln!("{error}");
                std::thread::sleep(Duration::from_secs(60 * 60));
            } else {
                std::thread::sleep(Duration::from_secs(60));
            }
        })
        .map_err(|error| format!("无法启动更新检查：{error}"))?;
    Ok(())
}

#[tauri::command]
pub async fn check_updates(app: AppHandle) -> Result<UpdateCheck, String> {
    tauri::async_runtime::spawn_blocking(move || check(&app, false))
        .await
        .map_err(|error| error.to_string())?
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Progress {
    downloaded: u64,
    total: u64,
}

fn download(app: &AppHandle, update: &Available, directory: &Path) -> Result<PathBuf, String> {
    let path = directory.join(format!(
        "clipstack-{}-{}.exe",
        update.info.version,
        uuid::Uuid::new_v4()
    ));
    let result = (|| {
        let mut response = client(900)?
            .get(&update.asset.browser_download_url)
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(|error| format!("下载失败，请重试。{error}"))?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| format!("无法保存安装包：{error}"))?;
        write_verified(
            &mut response,
            &mut file,
            &update.asset,
            |downloaded, total| {
                let _ = app.emit("update-progress", Progress { downloaded, total });
            },
        )?;
        file.sync_all()
            .map_err(|error| format!("无法保存安装包：{error}"))?;
        Ok(path.clone())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&path);
    }
    result
}

fn write_verified(
    reader: &mut impl Read,
    file: &mut impl Write,
    asset: &Asset,
    mut progress: impl FnMut(u64, u64),
) -> Result<(), String> {
    let mut digest = Sha256::new();
    let mut downloaded = 0u64;
    let mut last_percentage = 101;
    let mut buffer = [0u8; 65536];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|error| format!("下载中断：{error}"))?;
        if count == 0 {
            break;
        }
        downloaded += count as u64;
        if downloaded > asset.size {
            return Err("下载内容大小与发布信息不符。".into());
        }
        file.write_all(&buffer[..count])
            .map_err(|error| format!("无法写入安装包：{error}"))?;
        digest.update(&buffer[..count]);
        let percentage = downloaded * 100 / asset.size;
        if percentage != last_percentage {
            progress(downloaded, asset.size);
            last_percentage = percentage;
        }
    }
    let expected = asset.digest.as_deref().unwrap_or_default();
    if downloaded != asset.size
        || format!("sha256:{:x}", digest.finalize()) != expected.to_ascii_lowercase()
    {
        return Err("安装包 SHA-256 或大小校验失败，请重新下载。".into());
    }
    Ok(())
}

fn launch_installer(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
    let operation: Vec<_> = "runas".encode_utf16().chain(Some(0)).collect();
    let path: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            path.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    if result <= 32 {
        return Err("安装未启动或已取消管理员授权，可重新点击下载安装。".into());
    }
    Ok(())
}

#[tauri::command]
pub async fn install_update(app: AppHandle, version: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let service = app.state::<UpdateService>();
        let update = {
            let mut state = service.state.lock().unwrap();
            if state.installing {
                return Err("正在下载安装，请稍候。".into());
            }
            let update = state
                .available
                .clone()
                .filter(|update| update.info.version == version)
                .ok_or("更新信息已变化，请重新检查。")?;
            state.installing = true;
            update
        };
        let result =
            download(&app, &update, &service.directory).and_then(|path| launch_installer(&path));
        service.state.lock().unwrap().installing = false;
        if result.is_ok() {
            app.exit(0);
        }
        result
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_fixes_across_both_tag_encodings_and_normal_versions() {
        assert!(version("v1.1.0fix2") > version("v1.1.0fix1"));
        assert_eq!(version("1.1.0fix2"), version("1.1.0+2"));
        assert!(version("v1.1.1") > version("v1.1.0fix99"));
        assert!(version("v2.0.0") > version("v1.9.9fix99"));
        assert!(version("v1.1.0-beta").is_none());
    }

    #[test]
    fn validates_release_asset_before_offering_installation() {
        let make_release = |url: &str| Release {
            tag_name: "v99.0.0fix1".into(),
            body: Some("更新日志".into()),
            draft: false,
            prerelease: false,
            assets: vec![Asset {
                name: "clipstack-99.0.0fix1-windows-x64-setup.exe".into(),
                browser_download_url: url.into(),
                size: 100,
                digest: Some(format!("sha256:{}", "a".repeat(64))),
            }],
        };
        let trusted = "https://github.com/marumaruYGR/clipstack/releases/download/v99.0.0fix1/clipstack-99.0.0fix1-windows-x64-setup.exe";
        assert!(available(make_release(trusted)).unwrap().is_some());
        assert!(available(make_release("https://example.com/setup.exe")).is_err());
        let mut draft = make_release(trusted);
        draft.draft = true;
        assert!(available(draft).unwrap().is_none());
    }

    #[test]
    fn rejects_truncated_oversized_or_corrupt_downloads() {
        let bytes = b"synthetic installer for verification";
        let asset = Asset {
            name: String::new(),
            browser_download_url: String::new(),
            size: bytes.len() as u64,
            digest: Some(format!("sha256:{:x}", Sha256::digest(bytes))),
        };
        let mut written = Vec::new();
        write_verified(&mut &bytes[..], &mut written, &asset, |_, _| {}).unwrap();
        assert_eq!(written, bytes);
        assert!(write_verified(&mut &bytes[1..], &mut Vec::new(), &asset, |_, _| {}).is_err());
        assert!(write_verified(&mut &[b'x'; 60][..], &mut Vec::new(), &asset, |_, _| {}).is_err());
        assert!(write_verified(
            &mut &vec![b'x'; bytes.len()][..],
            &mut Vec::new(),
            &asset,
            |_, _| {}
        )
        .is_err());
    }
}
