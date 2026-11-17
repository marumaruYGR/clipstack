use std::{
    ffi::OsString,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    ptr::null_mut,
};

use windows::{
    core::{w, Interface, HSTRING, PCWSTR},
    Data::Xml::Dom::XmlDocument,
    Win32::{
        Foundation::{PROPERTYKEY, RPC_E_CHANGED_MODE},
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IPersistFile,
            StructuredStorage::PROPVARIANT, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
            STGM_READ,
        },
        UI::Shell::{
            Common::{IObjectArray, IObjectCollection, ITEMIDLIST},
            DestinationList, EnumerableObjectCollection, FOLDERID_Desktop, FOLDERID_Programs,
            ICustomDestinationList, ILFree, IShellLinkW,
            PropertiesSystem::IPropertyStore,
            SHGetKnownFolderPath, SHOpenFolderAndSelectItems, SHParseDisplayName,
            SetCurrentProcessExplicitAppUserModelID, ShellLink, KF_FLAG_DONT_VERIFY,
        },
    },
    UI::Notifications::{ToastNotification, ToastNotificationManager},
};

const APP_ID: PCWSTR = w!("cn.siyanghuasangou.clipstack");
const APP_ID_KEY: PROPERTYKEY = PROPERTYKEY {
    fmtid: windows::core::GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
    pid: 5,
};
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\剪栈";

pub fn configure_user_uninstall(install: bool) -> std::io::Result<()> {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    let exe = std::env::current_exe()?;
    let dir = exe
        .parent()
        .ok_or_else(|| std::io::Error::other("无法确定剪栈安装目录"))?;
    let uninstall = format!("\"{}\"", dir.join("uninstall.exe").display());
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if install {
        let (key, _) = hkcu.create_subkey(UNINSTALL_KEY)?;
        for (name, value) in [
            ("DisplayName", "剪栈".to_string()),
            ("DisplayVersion", env!("CARGO_PKG_VERSION").to_string()),
            ("Publisher", "四氧化三狗".to_string()),
            ("DisplayIcon", format!("\"{}\",0", exe.display())),
            ("InstallLocation", format!("\"{}\"", dir.display())),
            ("UninstallString", uninstall),
        ] {
            key.set_value(name, &value)?;
        }
        key.set_value("NoModify", &1_u32)?;
        key.set_value("NoRepair", &1_u32)?;
    } else if hkcu
        .open_subkey(UNINSTALL_KEY)
        .and_then(|key| key.get_value::<String, _>("UninstallString"))
        .is_ok_and(|value| value.eq_ignore_ascii_case(&uninstall))
    {
        hkcu.delete_subkey_all(UNINSTALL_KEY)?;
    }
    Ok(())
}

struct ComApartment(bool);

impl ComApartment {
    fn enter() -> windows::core::Result<Self> {
        let status = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if status == RPC_E_CHANGED_MODE {
            Ok(Self(false))
        } else {
            status.ok().map(|_| Self(true))
        }
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe {
                CoUninitialize();
            }
        }
    }
}

pub fn set_app_id() {
    let _ = unsafe { SetCurrentProcessExplicitAppUserModelID(APP_ID) };
}

pub fn show_paste_mode_tip(direct: bool) -> windows::core::Result<()> {
    let message = if direct {
        "点选的内容会直接粘贴到原来的窗口。想自己粘贴，可在剪栈设置中切换为「手动粘贴」。"
    } else {
        "点选后可在目标窗口按 Ctrl+V 粘贴。想直接上屏，可在剪栈设置中切换为「直接上屏」。"
    };
    if direct {
        show_system_notice("剪栈 · 直接上屏", message, "paste-direct")
    } else {
        show_system_notice("剪栈 · 手动粘贴", message, "paste-manual")
    }
}

pub fn show_welcome_1_1() -> windows::core::Result<()> {
    show_system_notice("剪栈", "庆祝剪栈迈入 1.1 版本时代！", "welcome-1.1")
}

fn show_system_notice(title: &str, message: &str, tag: &str) -> windows::core::Result<()> {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};

    // Register the sender even when the optional Start menu shortcut was skipped.
    let (identity, _) = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey(r"Software\Classes\AppUserModelId\cn.siyanghuasangou.clipstack")?;
    identity.set_value("DisplayName", &"剪栈")?;

    let _apartment = ComApartment::enter()?;
    let xml = XmlDocument::new()?;
    xml.LoadXml(&HSTRING::from(format!(
        "<toast><visual><binding template=\"ToastGeneric\"><text>{title}</text><text>{message}</text></binding></visual><audio silent=\"true\"/></toast>",
    )))?;
    let toast = ToastNotification::CreateToastNotification(&xml)?;
    toast.SetTag(&HSTRING::from(tag))?;
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(
        "cn.siyanghuasangou.clipstack",
    ))?
    .Show(&toast)
}

pub fn signed_in_account_matches(expected: &str) -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, LocalFree},
        Security::{
            Authorization::ConvertSidToStringSidW, GetTokenInformation, TokenUser, TOKEN_QUERY,
            TOKEN_USER,
        },
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    unsafe {
        let mut token = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut bytes = 0;
        GetTokenInformation(token, TokenUser, null_mut(), 0, &mut bytes);
        let mut buffer = vec![0usize; (bytes as usize).div_ceil(std::mem::size_of::<usize>())];
        let status = GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            bytes,
            &mut bytes,
        );
        CloseHandle(token);
        if status == 0 || bytes < std::mem::size_of::<TOKEN_USER>() as u32 {
            return false;
        }
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        let mut sid = null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut sid) == 0 {
            return false;
        }
        let actual = PCWSTR(sid).to_string();
        LocalFree(sid.cast());
        actual.is_ok_and(|actual| actual == expected)
    }
}

pub fn configure_user_shortcuts(
    start_menu: bool,
    desktop: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let _apartment = ComApartment::enter()?;
    let exe = std::env::current_exe()?;
    for (folder, enabled) in [(FOLDERID_Programs, start_menu), (FOLDERID_Desktop, desktop)] {
        let folder = known_folder(&folder)?;
        let shortcut = folder.join("剪栈.lnk");
        if shortcut.exists() && !is_clipstack_shortcut(&shortcut) {
            if enabled {
                return Err(format!("快捷方式已存在：{}", shortcut.display()).into());
            }
            continue;
        }
        if enabled {
            std::fs::create_dir_all(&folder)?;
            write_shortcut(&shortcut, &exe)?;
        } else if let Err(error) = std::fs::remove_file(&shortcut) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(error.into());
            }
        }
    }
    Ok(())
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn known_folder(id: &windows::core::GUID) -> windows::core::Result<PathBuf> {
    unsafe {
        let folder = SHGetKnownFolderPath(id, KF_FLAG_DONT_VERIFY, None)?;
        let path = PathBuf::from(OsString::from_wide(folder.as_wide()));
        CoTaskMemFree(Some(folder.as_ptr().cast()));
        Ok(path)
    }
}

fn is_clipstack_shortcut(shortcut: &Path) -> bool {
    let app_id = || -> windows::core::Result<windows::core::BSTR> {
        unsafe {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            let file: IPersistFile = link.cast()?;
            file.Load(PCWSTR(wide(shortcut).as_ptr()), STGM_READ)?;
            let properties: IPropertyStore = link.cast()?;
            windows::core::BSTR::try_from(&properties.GetValue(&APP_ID_KEY)?)
        }
    }();
    app_id.is_ok_and(|id| id.to_string() == "cn.siyanghuasangou.clipstack")
}

fn write_shortcut(shortcut: &Path, exe: &Path) -> windows::core::Result<()> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let path = wide(exe);
        link.SetPath(PCWSTR(path.as_ptr()))?;
        link.SetIconLocation(PCWSTR(path.as_ptr()), 0)?;
        let props: IPropertyStore = link.cast()?;
        props.SetValue(
            &APP_ID_KEY,
            &PROPVARIANT::from("cn.siyanghuasangou.clipstack"),
        )?;
        props.Commit()?;
        let file: IPersistFile = link.cast()?;
        file.Save(PCWSTR(wide(shortcut).as_ptr()), true)
    }
}

pub fn register_search_task() -> Result<(), Box<dyn std::error::Error>> {
    let _apartment = ComApartment::enter()?;
    let path = std::env::current_exe()?;
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(PCWSTR(path.as_ptr()))?;
        link.SetArguments(w!("--search"))?;
        link.SetIconLocation(PCWSTR(path.as_ptr()), 0)?;
        let props: IPropertyStore = link.cast()?;
        let title = PROPERTYKEY {
            fmtid: windows::core::GUID::from_u128(0xf29f85e0_4ff9_1068_ab91_08002b27b3d9),
            pid: 2,
        };
        let value = PROPVARIANT::from("检索剪贴板");
        props.SetValue(&title, &value)?;
        props.Commit()?;
        let objects: IObjectCollection =
            CoCreateInstance(&EnumerableObjectCollection, None, CLSCTX_INPROC_SERVER)?;
        objects.AddObject(&link)?;
        let tasks: IObjectArray = objects.cast()?;
        let list: ICustomDestinationList =
            CoCreateInstance(&DestinationList, None, CLSCTX_INPROC_SERVER)?;
        list.SetAppID(APP_ID)?;
        let _: IObjectArray = list.BeginList(&mut 0)?;
        if let Err(error) = list.AddUserTasks(&tasks).and_then(|_| list.CommitList()) {
            let _ = list.AbortList();
            return Err(error.into());
        }
    }
    Ok(())
}

pub fn locate(path: &Path) -> Result<(), String> {
    if !path.is_absolute() || !path.exists() {
        return Err("此文件已移动、删除或当前无法访问。".into());
    }
    let _apartment = ComApartment::enter().map_err(|error| error.to_string())?;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut item: *mut ITEMIDLIST = null_mut();
    unsafe {
        SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut item, 0, None)
            .map_err(|error| error.to_string())?;
        let result = SHOpenFolderAndSelectItems(item, None, 0);
        ILFree(Some(item));
        result.map_err(|error| format!("无法在资源管理器中定位：{error}"))
    }
}
