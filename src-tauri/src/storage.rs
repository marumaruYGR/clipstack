use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::mem::size_of;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use uuid::Uuid;
use windows_sys::core::GUID;
use windows_sys::Win32::Foundation::{CloseHandle, LocalFree};
use windows_sys::Win32::Security::Cryptography::{
    BCryptGenRandom, CryptHashCertificate2, CryptProtectData, CryptProtectMemory,
    CryptUnprotectData, CryptUnprotectMemory, BCRYPT_SHA256_ALGORITHM,
    BCRYPT_USE_SYSTEM_PREFERRED_RNG, CRYPTPROTECTMEMORY_SAME_LOGON, CRYPTPROTECT_UI_FORBIDDEN,
    CRYPT_INTEGER_BLOB,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenSessionId, TokenStatistics, TOKEN_QUERY, TOKEN_STATISTICS,
};
use windows_sys::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows_sys::Win32::UI::Shell::{
    FOLDERID_Documents, FOLDERID_LocalAppData, SHGetKnownFolderPath, KF_FLAG_DONT_VERIFY,
};

const FAVORITES_FILE: &str = "favorites.dpapi";
const HISTORY_MAGIC: &[u8; 8] = b"JZHIST01";
const KEY_MARKER: &[u8; 16] = b"ClipstackLogon01";
const ENTROPY_LENGTH: usize = 32;
const HEADER_LENGTH: usize = HISTORY_MAGIC.len() + size_of::<u64>() + ENTROPY_LENGTH;

pub struct Storage {
    root: PathBuf,
    history: PathBuf,
    logon: u64,
}

#[derive(Deserialize, Serialize)]
struct MoveJournal {
    old: PathBuf,
    new: PathBuf,
    directory: PathBuf,
    digest: [u8; 32],
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FavoritesLocation {
    #[serde(default)]
    favorites_directory: PathBuf,
}

pub fn default_favorites_dir() -> Result<PathBuf, String> {
    Ok(known_folder(&FOLDERID_Documents)?.join("剪栈"))
}

impl Storage {
    pub fn open() -> Result<Self, String> {
        let root = known_folder(&FOLDERID_LocalAppData)?.join("剪栈");
        fs::create_dir_all(&root).map_err(|e| format!("无法创建剪栈数据目录：{e}"))?;
        let (session, logon) = logon_session()?;
        let storage = Self {
            history: root.join(format!("history-{session}.dpapi")),
            root,
            logon,
        };
        // A drive from a previous move may currently be disconnected. Keep both the
        // committed settings and the journal so the cleanup can be retried later.
        let _ = storage.recover_move();
        Ok(storage)
    }

    pub fn load_settings<T: DeserializeOwned + Default>(&self) -> Result<T, String> {
        let Some(bytes) = read_if_present(&self.settings_file())? else {
            return Ok(T::default());
        };
        decode(&bytes)
    }

    pub fn save_settings<T: Serialize>(&self, value: &T) -> Result<(), String> {
        let _ = self.recover_move();
        atomic_write(&self.settings_file(), &encode(value)?, true)
    }

    pub fn load_history<T: DeserializeOwned + Default>(&self) -> Result<T, String> {
        let Some(bytes) = read_if_present(&self.history)? else {
            return Ok(T::default());
        };
        let Some((header, encrypted)) = bytes.split_at_checked(HEADER_LENGTH) else {
            return Err("剪贴板历史文件不完整".into());
        };
        if !header.starts_with(HISTORY_MAGIC) {
            return Err("剪贴板历史文件格式无效".into());
        }
        let key_start = HISTORY_MAGIC.len() + size_of::<u64>();
        if header[HISTORY_MAGIC.len()..key_start] != self.logon.to_le_bytes() {
            return self.expire_history();
        }

        // Authentication IDs can be reused after reboot. The wrapped key also
        // depends on the boot and logon, so an old file cannot be recovered.
        let mut key = [0; ENTROPY_LENGTH];
        key.copy_from_slice(&header[key_start..]);
        if !transform_session_key(&mut key, false) || !key.starts_with(KEY_MARKER) {
            wipe(&mut key);
            return self.expire_history();
        }
        let plain = dpapi(encrypted, Some(&key), false);
        wipe(&mut key);
        decode_private(plain?)
    }

    pub fn save_history<T: Serialize>(&self, value: &T) -> Result<(), String> {
        let mut key = [0; ENTROPY_LENGTH];
        key[..KEY_MARKER.len()].copy_from_slice(KEY_MARKER);
        let random = &mut key[KEY_MARKER.len()..];
        let status = unsafe {
            BCryptGenRandom(
                null_mut(),
                random.as_mut_ptr(),
                random.len() as u32,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        if status < 0 {
            return Err(format!("无法生成会话密钥：0x{:08X}", status as u32));
        }
        let mut plain = encode(value)?;
        let encrypted = dpapi(&plain, Some(&key), true);
        wipe(&mut plain);
        let wrapped = transform_session_key(&mut key, true);
        if !wrapped {
            wipe(&mut key);
            return Err(format!("无法保护会话密钥：{}", io::Error::last_os_error()));
        }
        let encrypted = encrypted?;
        let mut bytes = Vec::with_capacity(HEADER_LENGTH + encrypted.len());
        bytes.extend_from_slice(HISTORY_MAGIC);
        bytes.extend_from_slice(&self.logon.to_le_bytes());
        bytes.extend_from_slice(&key);
        bytes.extend_from_slice(&encrypted);
        atomic_write(&self.history, &bytes, true)
    }

    pub fn clear_session_history(&self) -> Result<(), String> {
        remove_if_present(&self.history)
    }

    pub fn load_favorites<T: DeserializeOwned + Default>(&self, dir: &Path) -> Result<T, String> {
        let Some(bytes) = read_if_present(&dir.join(FAVORITES_FILE))? else {
            return Ok(T::default());
        };
        decode_private(dpapi(&bytes, None, false)?)
    }

    pub fn save_favorites<T: Serialize>(&self, dir: &Path, value: &T) -> Result<(), String> {
        fs::create_dir_all(dir).map_err(|e| format!("无法创建收藏夹目录：{e}"))?;
        let mut plain = encode(value)?;
        let encrypted = dpapi(&plain, None, true);
        wipe(&mut plain);
        atomic_write(&dir.join(FAVORITES_FILE), &encrypted?, true)
    }

    pub fn save_recovered_favorites<T: Serialize, S: Serialize>(
        &self,
        dir: &Path,
        entries: &T,
        settings: &S,
    ) -> Result<(), String> {
        let parent = dir.parent().ok_or("恢复目录缺少父目录。")?;
        fs::create_dir_all(parent).map_err(|e| format!("无法创建恢复位置：{e}"))?;
        fs::create_dir(dir).map_err(|e| format!("无法新建收藏恢复目录：{e}"))?;
        let result = self
            .save_favorites(dir, entries)
            .and_then(|_| self.save_settings(settings));
        if result.is_err() {
            let _ = remove_if_present(&dir.join(FAVORITES_FILE));
            let _ = fs::remove_dir(dir);
        }
        result
    }

    pub fn move_favorites<T: Serialize>(
        &self,
        old_dir: &Path,
        new_dir: &Path,
        settings: &T,
    ) -> Result<(), String> {
        self.recover_move()?;
        if !new_dir.is_absolute() {
            return Err("请选择 Windows 绝对路径".into());
        }
        fs::create_dir_all(new_dir).map_err(|e| format!("无法创建目标目录：{e}"))?;
        let new = fs::canonicalize(new_dir)
            .map_err(|e| format!("无法访问目标目录：{e}"))?
            .join(FAVORITES_FILE);
        let old = match fs::canonicalize(old_dir) {
            Ok(dir) => Some(dir.join(FAVORITES_FILE)),
            Err(e)
                if e.kind() == io::ErrorKind::NotFound && old_dir == default_favorites_dir()? =>
            {
                None
            }
            Err(e) => return Err(format!("无法访问原收藏夹目录：{e}")),
        };
        if old.as_ref() == Some(&new) {
            return self.save_settings(settings);
        }
        if new
            .try_exists()
            .map_err(|e| format!("无法检查目标文件：{e}"))?
        {
            return Err("目标目录已包含收藏文件，请选择其他目录".into());
        }
        let Some(old) = old else {
            return self.save_settings(settings);
        };
        let Some(bytes) = read_if_present(&old)? else {
            return self.save_settings(settings);
        };

        let settings_bytes = encode(settings)?;
        let location: FavoritesLocation = decode(&settings_bytes)?;
        if location.favorites_directory != new_dir {
            return Err("新收藏夹目录与设置不一致".into());
        }
        let record = MoveJournal {
            old,
            new,
            directory: new_dir.to_path_buf(),
            digest: digest(&bytes)?,
        };
        atomic_write(&self.move_journal(), &encode(&record)?, false)?;
        if let Err(error) = atomic_write(&record.new, &bytes, false) {
            let _ = remove_if_present(&self.move_journal());
            return Err(error);
        }
        if let Err(error) = atomic_write(&self.settings_file(), &settings_bytes, true) {
            let _ = self.recover_move();
            return Err(error);
        }

        // Settings now point to the new copy. If deletion is temporarily
        // blocked, the journal protects recovery across a process restart.
        let _ = self.recover_move();
        Ok(())
    }

    fn expire_history<T: Default>(&self) -> Result<T, String> {
        self.clear_session_history()?;
        Ok(T::default())
    }

    fn settings_file(&self) -> PathBuf {
        self.root.join("settings.json")
    }

    fn move_journal(&self) -> PathBuf {
        self.root.join("favorites-move.json")
    }

    fn recover_move(&self) -> Result<(), String> {
        let journal = self.move_journal();
        let Some(bytes) = read_if_present(&journal)? else {
            return Ok(());
        };
        let record: MoveJournal = decode(&bytes)?;
        let active = read_if_present(&self.settings_file())?
            .map(|bytes| decode::<FavoritesLocation>(&bytes))
            .transpose()?;
        let committed =
            active.is_some_and(|location| location.favorites_directory == record.directory);
        let (keep, remove) = if committed {
            (&record.new, &record.old)
        } else {
            (&record.old, &record.new)
        };
        let Some(remove_bytes) = read_if_present(remove)? else {
            let parent = remove.parent().ok_or("收藏文件缺少目录")?;
            fs::metadata(parent).map_err(|e| format!("暂时无法访问收藏文件目录：{e}"))?;
            return remove_if_present(&journal);
        };
        if digest(&remove_bytes)? != record.digest {
            return remove_if_present(&journal);
        }
        let keep_bytes = read_if_present(keep)?.ok_or("暂时无法访问迁移后的收藏文件")?;
        if digest(&keep_bytes)? != record.digest {
            let mut decoded = dpapi(&keep_bytes, None, false)?;
            wipe(&mut decoded);
        }
        remove_if_present(remove)?;
        remove_if_present(&journal)
    }
}

fn digest(bytes: &[u8]) -> Result<[u8; 32], String> {
    let mut out = [0; 32];
    let mut length = out.len() as u32;
    let input_length = u32::try_from(bytes.len()).map_err(|_| "收藏文件过大")?;
    let ok = unsafe {
        CryptHashCertificate2(
            BCRYPT_SHA256_ALGORITHM,
            0,
            null(),
            bytes.as_ptr(),
            input_length,
            out.as_mut_ptr(),
            &mut length,
        )
    };
    if ok == 0 {
        return Err(format!("无法校验收藏文件：{}", io::Error::last_os_error()));
    }
    Ok(out)
}

fn known_folder(id: &GUID) -> Result<PathBuf, String> {
    let mut raw = null_mut();
    let status =
        unsafe { SHGetKnownFolderPath(id, KF_FLAG_DONT_VERIFY as u32, null_mut(), &mut raw) };
    let result = if status < 0 || raw.is_null() {
        Err(format!(
            "无法获取 Windows 用户目录：0x{:08X}",
            status as u32
        ))
    } else {
        let mut length = 0;
        unsafe {
            while *raw.add(length) != 0 {
                length += 1;
            }
            let units = std::slice::from_raw_parts(raw, length);
            Ok(PathBuf::from(std::ffi::OsString::from_wide(units)))
        }
    };
    unsafe { CoTaskMemFree(raw.cast()) };
    result
}

fn logon_session() -> Result<(u32, u64), String> {
    unsafe {
        let mut token = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(format!(
                "无法打开 Windows 登录令牌：{}",
                io::Error::last_os_error()
            ));
        }
        let mut statistics = TOKEN_STATISTICS::default();
        let mut session = 0u32;
        let mut length = 0;
        let ok = GetTokenInformation(
            token,
            TokenStatistics,
            (&mut statistics as *mut TOKEN_STATISTICS).cast(),
            size_of::<TOKEN_STATISTICS>() as u32,
            &mut length,
        ) != 0
            && GetTokenInformation(
                token,
                TokenSessionId,
                (&mut session as *mut u32).cast(),
                size_of::<u32>() as u32,
                &mut length,
            ) != 0;
        let error = io::Error::last_os_error();
        CloseHandle(token);
        if !ok {
            return Err(format!("无法读取 Windows 登录会话：{error}"));
        }
        let id = statistics.AuthenticationId;
        Ok((
            session,
            (u64::from(id.HighPart as u32) << 32) | u64::from(id.LowPart),
        ))
    }
}

fn transform_session_key(key: &mut [u8; ENTROPY_LENGTH], encrypt: bool) -> bool {
    let transform = if encrypt {
        CryptProtectMemory
    } else {
        CryptUnprotectMemory
    };
    unsafe {
        transform(
            key.as_mut_ptr().cast(),
            ENTROPY_LENGTH as u32,
            CRYPTPROTECTMEMORY_SAME_LOGON,
        ) != 0
    }
}

fn dpapi(bytes: &[u8], entropy: Option<&[u8]>, encrypt: bool) -> Result<Vec<u8>, String> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(bytes.len()).map_err(|_| "数据过大，无法使用 Windows 数据保护")?,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let entropy = entropy.map(|key| CRYPT_INTEGER_BLOB {
        cbData: key.len() as u32,
        pbData: key.as_ptr().cast_mut(),
    });
    let entropy = entropy.as_ref().map_or(null(), |key| key as *const _);
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe {
        let ok = if encrypt {
            CryptProtectData(
                &input,
                null(),
                entropy,
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        } else {
            CryptUnprotectData(
                &input,
                null_mut(),
                entropy,
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        if ok == 0 {
            let verb = if encrypt { "加密" } else { "解密" };
            return Err(format!(
                "Windows 数据{verb}失败：{}",
                io::Error::last_os_error()
            ));
        }
        if out.pbData.is_null() {
            return Ok(Vec::new());
        }
        let data = std::slice::from_raw_parts_mut(out.pbData, out.cbData as usize);
        let copied = data.to_vec();
        if !encrypt {
            wipe(data);
        }
        LocalFree(out.pbData.cast());
        Ok(copied)
    }
}

fn wipe(bytes: &mut [u8]) {
    for byte in bytes {
        unsafe { std::ptr::write_volatile(byte, 0) };
    }
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    serde_json::to_vec(value).map_err(|e| format!("无法序列化剪栈数据：{e}"))
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    serde_json::from_slice(bytes).map_err(|e| format!("无法读取剪栈数据：{e}"))
}

fn decode_private<T: DeserializeOwned>(mut bytes: Vec<u8>) -> Result<T, String> {
    let decoded = decode(&bytes);
    wipe(&mut bytes);
    decoded
}

fn read_if_present(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("无法读取 {}：{e}", path.display())),
    }
}

fn remove_if_present(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("无法移除 {}：{e}", path.display())),
    }
}

fn atomic_write(path: &Path, bytes: &[u8], replace: bool) -> Result<(), String> {
    write_atomic(path, bytes, replace, false)
}

pub(crate) fn write_backup_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    write_atomic(path, bytes, true, true)
}

fn write_atomic(path: &Path, bytes: &[u8], replace: bool, verify: bool) -> Result<(), String> {
    let dir = fs::canonicalize(path.parent().ok_or("剪栈数据路径缺少目录")?)
        .map_err(|e| format!("无法打开数据目录：{e}"))?;
    let path = dir.join(path.file_name().ok_or("剪栈数据路径缺少文件名")?);
    let temporary = dir.join(format!(".clipstack-{}.tmp", Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|e| format!("无法创建数据临时文件：{e}"))?;
    let result = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    let result = result.and_then(|()| {
        if verify {
            let mut written = fs::File::open(&temporary)?;
            let mut buffer = [0; 8192];
            for expected in bytes.chunks(buffer.len()) {
                written.read_exact(&mut buffer[..expected.len()])?;
                if &buffer[..expected.len()] != expected {
                    return Err(io::Error::other("备份文件回读校验失败"));
                }
            }
            if written.read(&mut buffer)? != 0 {
                return Err(io::Error::other("备份文件大小与写入内容不一致"));
            }
        }
        let source: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
        let destination: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let flags = MOVEFILE_WRITE_THROUGH
            | if replace {
                MOVEFILE_REPLACE_EXISTING
            } else {
                0
            };
        if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), flags) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    });
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|e| format!("无法保存 {}：{e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_writes_a_new_dpapi_store_and_keeps_the_old_file() {
        let root = std::env::temp_dir().join(format!("clipstack-recovery-test-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("old")).unwrap();
        let old = root.join("old").join(FAVORITES_FILE);
        fs::write(&old, b"unreadable previous installation").unwrap();
        let storage = Storage {
            history: root.join("history.dpapi"),
            root: root.clone(),
            logon: 0,
        };
        let destination = root.join("recovered");
        let recovered = vec!["已恢复".to_string()];
        let settings = serde_json::json!({ "favoritesDirectory": destination });

        assert!(storage
            .load_favorites::<Vec<String>>(&root.join("old"))
            .is_err());
        storage
            .save_recovered_favorites(&destination, &recovered, &settings)
            .unwrap();
        assert_eq!(fs::read(&old).unwrap(), b"unreadable previous installation");
        assert_eq!(
            storage.load_favorites::<Vec<String>>(&destination).unwrap(),
            recovered
        );
        assert_eq!(
            storage.load_settings::<serde_json::Value>().unwrap(),
            settings
        );

        fs::remove_file(old).unwrap();
        fs::remove_dir(root.join("old")).unwrap();
        fs::remove_file(destination.join(FAVORITES_FILE)).unwrap();
        fs::remove_dir(destination).unwrap();
        fs::remove_file(storage.settings_file()).unwrap();
        fs::remove_dir(root).unwrap();
    }
}
