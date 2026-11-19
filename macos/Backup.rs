use std::{ffi::{c_char, CString}, ptr, slice};

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{aead::AeadInPlace, KeyInit, Tag, XChaCha20Poly1305, XNonce};
use zeroize::{Zeroize, Zeroizing};

const MAGIC: &[u8; 8] = b"JZBACK01";
const HEADER: usize = 48;
const TAG: usize = 16;
const LIMIT: usize = 512 * 1024 * 1024;

fn cipher(password: &[u8], salt: &[u8]) -> Result<XChaCha20Poly1305, &'static str> {
    let params = Params::new(64 * 1024, 3, 4, Some(32)).map_err(|_| "无法配置备份加密。")?;
    let mut key = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password, salt, &mut *key)
        .map_err(|_| "无法生成备份密钥。")?;
    XChaCha20Poly1305::new_from_slice(key.as_slice()).map_err(|_| "无法配置备份密钥。")
}

fn seal(input: &[u8], password: &[u8], random: &[u8]) -> Result<Vec<u8>, &'static str> {
    let text = std::str::from_utf8(password).map_err(|_| "备份口令无效。")?;
    if text.chars().count() < 12 || password.len() > 1024 || text.trim().is_empty() {
        return Err("备份口令需要至少 12 个字符，最多 1024 字节。");
    }
    if input.is_empty() || input.len() > LIMIT - HEADER - TAG || random.len() != 40 {
        return Err("收藏备份大小超过 512 MiB，或随机数据无效。");
    }
    let mut bytes = Vec::with_capacity(HEADER + input.len() + TAG);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(random);
    bytes.extend_from_slice(input);
    let (header, body) = bytes.split_at_mut(HEADER);
    let tag = cipher(password, &header[8..24])?
        .encrypt_in_place_detached(XNonce::from_slice(&header[24..48]), header, body)
        .map_err(|_| "无法加密收藏备份。")?;
    bytes.extend_from_slice(&tag);
    Ok(bytes)
}

fn open(input: &[u8], password: &[u8]) -> Result<Vec<u8>, &'static str> {
    if password.is_empty() || password.len() > 1024 {
        return Err("请输入有效的备份口令。");
    }
    if input.len() <= HEADER + TAG || input.len() > LIMIT || !input.starts_with(MAGIC) {
        return Err("备份格式无效、版本不受支持或文件不完整。");
    }
    let mut bytes = Zeroizing::new(input[HEADER..input.len() - TAG].to_vec());
    cipher(password, &input[8..24])?
        .decrypt_in_place_detached(
            XNonce::from_slice(&input[24..48]), &input[..HEADER], &mut bytes,
            Tag::from_slice(&input[input.len() - TAG..]),
        )
        .map_err(|_| "备份口令错误，或文件已损坏。")?;
    Ok(std::mem::take(&mut *bytes))
}

/// Operation 0 seals JSON with 40 securely generated salt/nonce bytes. Operation 1 opens a backup.
#[no_mangle]
pub unsafe extern "C" fn stack_backup(
    operation: u32, input: *const u8, count: usize, password: *const u8, password_count: usize,
    random: *const u8, result_count: *mut usize, error: *mut *mut c_char,
) -> *mut u8 {
    if result_count.is_null() || error.is_null() {
        return ptr::null_mut();
    }
    *result_count = 0;
    *error = ptr::null_mut();
    let result = if input.is_null() || password.is_null() || count > LIMIT || password_count > 1024 {
        Err("备份参数无效。")
    } else {
        let data = slice::from_raw_parts(input, count);
        let secret = Zeroizing::new(slice::from_raw_parts(password, password_count).to_vec());
        match operation {
            0 if !random.is_null() => seal(data, &secret, slice::from_raw_parts(random, 40)),
            1 => open(data, &secret),
            _ => Err("备份操作无效。"),
        }
    };
    match result {
        Ok(bytes) => {
            let mut boxed = bytes.into_boxed_slice();
            *result_count = boxed.len();
            let pointer = boxed.as_mut_ptr();
            std::mem::forget(boxed);
            pointer
        }
        Err(message) => {
            *error = CString::new(message).unwrap().into_raw();
            ptr::null_mut()
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn stack_free_bytes(bytes: *mut u8, count: usize) {
    if !bytes.is_null() {
        let mut boxed = Box::from_raw(ptr::slice_from_raw_parts_mut(bytes, count));
        boxed.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authenticates_portable_header_password_and_payload() {
        let json = br#"[{"id":"a","created_at":123,"content":{"kind":"text","text":"hello"}}]"#;
        let password = b"twelve characters or more";
        let random = [19; 40];
        let encrypted = seal(json, password, &random).unwrap();
        assert_eq!(&encrypted[..8], b"JZBACK01");
        assert_eq!(open(&encrypted, password).unwrap(), json);
        assert!(open(&encrypted, b"wrong password").is_err());
        for index in [8, 24, 48, encrypted.len() - 1] {
            let mut corrupt = encrypted.clone();
            corrupt[index] ^= 1;
            assert!(open(&corrupt, password).is_err());
        }
        assert!(seal(json, b"short", &random).is_err());
        assert!(open(b"JZBACK01", password).is_err());
    }
}
