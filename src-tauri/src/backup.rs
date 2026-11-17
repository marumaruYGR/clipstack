use std::{fs::File, io::Read, path::Path};

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{aead::AeadInPlace, KeyInit, Tag, XChaCha20Poly1305, XNonce};
use windows_sys::Win32::Security::Cryptography::{
    BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
};
use zeroize::Zeroizing;

use crate::{model::Entry, storage::write_backup_file};

pub const EXTENSION: &str = "clipstack-backup";
// Version 1 fixes the KDF and cipher parameters, so an input file cannot select a more costly KDF.
const MAGIC: &[u8; 8] = b"JZBACK01";
const NONCE_START: usize = MAGIC.len() + 16;
const HEADER_SIZE: usize = NONCE_START + 24;
const TAG_SIZE: usize = 16;
const MAX_BYTES: usize = 512 * 1024 * 1024;
const MAX_ENTRIES: usize = 50_000;

pub fn export(path: &Path, password: &str, entries: &[Entry]) -> Result<usize, String> {
    check_path(path)?;
    if password.chars().count() < 12 || password.len() > 1024 || password.trim().is_empty() {
        return Err("备份口令需要至少 12 个字符，最多 1024 字节。".into());
    }
    let bytes = encrypt(password, entries)?;
    write_backup_file(path, &bytes)?;
    Ok(entries.len())
}

pub fn read(path: &Path, password: &str) -> Result<Vec<Entry>, String> {
    check_path(path)?;
    if password.is_empty() || password.len() > 1024 {
        return Err("请输入有效的备份口令。".into());
    }
    let file = File::open(path).map_err(|e| format!("无法打开备份文件：{e}"))?;
    let metadata = file
        .metadata()
        .map_err(|e| format!("无法检查备份文件：{e}"))?;
    if !metadata.is_file() || metadata.len() > MAX_BYTES as u64 {
        return Err("请选择 512 MiB 以内的剪栈备份文件。".into());
    }
    let mut bytes = Zeroizing::new(Vec::new());
    file.take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("无法读取备份文件：{e}"))?;
    decrypt(password, &mut bytes)
}

fn check_path(path: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || !path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case(EXTENSION))
    {
        return Err("请选择 .clipstack-backup 格式的剪栈加密备份文件。".into());
    }
    Ok(())
}

fn cipher(password: &str, salt: &[u8]) -> Result<XChaCha20Poly1305, String> {
    let params = Params::new(64 * 1024, 3, 4, Some(32)).map_err(|e| e.to_string())?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(password.as_bytes(), salt, &mut *key)
        .map_err(|e| format!("无法生成备份密钥：{e}"))?;
    XChaCha20Poly1305::new_from_slice(key.as_slice()).map_err(|e| e.to_string())
}

fn encrypt(password: &str, entries: &[Entry]) -> Result<Zeroizing<Vec<u8>>, String> {
    if entries.is_empty() || entries.len() > MAX_ENTRIES {
        return Err("可导出的收藏数量必须在 1 到 50,000 条之间。".into());
    }
    let mut bytes = Zeroizing::new(vec![0; HEADER_SIZE]);
    serde_json::to_writer(&mut *bytes, entries).map_err(|e| format!("无法整理收藏：{e}"))?;
    if bytes.len() > MAX_BYTES - TAG_SIZE {
        return Err("收藏备份大小超过 512 MiB。".into());
    }
    let (header, body) = bytes.split_at_mut(HEADER_SIZE);
    header[..MAGIC.len()].copy_from_slice(MAGIC);
    let random = &mut header[MAGIC.len()..];
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            random.as_mut_ptr(),
            random.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status != 0 {
        return Err("无法为备份生成安全的随机数据。".into());
    }
    let cipher = cipher(password, &header[MAGIC.len()..NONCE_START])?;
    let tag = cipher
        .encrypt_in_place_detached(XNonce::from_slice(&header[NONCE_START..]), header, body)
        .map_err(|_| "无法加密收藏备份。".to_string())?;
    bytes.extend_from_slice(&tag);
    Ok(bytes)
}

fn decrypt(password: &str, bytes: &mut [u8]) -> Result<Vec<Entry>, String> {
    if bytes.len() <= HEADER_SIZE + TAG_SIZE || bytes.len() > MAX_BYTES || !bytes.starts_with(MAGIC)
    {
        return Err("备份格式无效、版本不受支持或文件不完整。".into());
    }
    let (header, encrypted) = bytes.split_at_mut(HEADER_SIZE);
    let (body, tag) = encrypted.split_at_mut(encrypted.len() - TAG_SIZE);
    let cipher = cipher(password, &header[MAGIC.len()..NONCE_START])?;
    cipher
        .decrypt_in_place_detached(
            XNonce::from_slice(&header[NONCE_START..]),
            header,
            body,
            Tag::from_slice(tag),
        )
        .map_err(|_| "备份口令错误，或文件已损坏。".to_string())?;
    let entries: Vec<Entry> =
        serde_json::from_slice(body).map_err(|_| "备份中的收藏记录格式无效。".to_string())?;
    if entries.is_empty()
        || entries.len() > MAX_ENTRIES
        || entries
            .iter()
            .any(|entry| entry.created_at > 8_640_000_000_000_000)
    {
        return Err("备份中的收藏记录数量或日期无效。".into());
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clipboard::Content;

    #[test]
    fn portable_backup_recovers_content_and_authenticates_bytes() {
        let entries = [
            Entry::new(Content::Text {
                text: "剪栈：测试 👋\n第二行".into(),
            }),
            Entry::new(Content::Files {
                paths: vec![r"D:\素材\文档.docx".into()],
                rich: None,
            }),
            Entry::new(Content::RichText {
                text: "加粗文本".into(),
                html: Some(b"<strong>Clipstack</strong>".to_vec()),
                rtf: Some(b"{\\rtf1 Clipstack}".to_vec()),
                url: None,
            }),
            Entry::new(Content::Image {
                png: include_bytes!("../../ui/assets/app.png").to_vec(),
                width: 128,
                height: 128,
                rich: None,
                bitmap: None,
            }),
        ];
        let password = "testing the backup passphrase";
        let sealed = encrypt(password, &entries).unwrap();
        assert!(!sealed
            .windows("第二行".len())
            .any(|bytes| bytes == "第二行".as_bytes()));
        let restored = decrypt(password, &mut sealed.to_vec()).unwrap();
        assert_eq!(restored.len(), entries.len());
        assert!(restored
            .iter()
            .zip(entries)
            .all(|(restored, entry)| restored.content == entry.content));
        assert!(decrypt("incorrect backup passphrase", &mut sealed.to_vec()).is_err());
        let mut tampered = sealed.to_vec();
        tampered[HEADER_SIZE] ^= 1;
        assert!(decrypt(password, &mut tampered).is_err());
    }
}
