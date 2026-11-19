use std::{
    collections::{HashMap, HashSet},
    ffi::{c_char, c_int, CStr, CString, OsStr},
    fs::{File, OpenOptions, TryLockError},
    os::unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    slice, str,
    sync::{LazyLock, Mutex},
};

#[path = "Backup.rs"]
mod backup;

#[derive(Default)]
struct Library {
    history: Vec<String>,
    favorites: Vec<String>,
    search_text: HashMap<String, String>,
}
static LIBRARY: LazyLock<Mutex<Library>> = LazyLock::new(|| Mutex::new(Library::default()));

impl Library {
    fn matches(&self, query: &str) -> String {
        let words: Vec<_> = query.split_whitespace().collect();
        let filter = |ids: &[String]| {
            ids.iter()
                .filter(|id| {
                    self.search_text
                        .get(*id)
                        .is_some_and(|text| words.iter().all(|word| text.contains(*word)))
                })
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(",")
        };
        format!("{}\n{}", filter(&self.history), filter(&self.favorites))
    }

    fn discard_unused_search_text(&mut self) {
        if self.search_text.is_empty() {
            return;
        }
        let live: HashSet<_> = self.history.iter().chain(&self.favorites).collect();
        self.search_text.retain(|id, _| live.contains(id));
    }
}

fn upsert(list: &mut Vec<String>, id: &str) {
    list.retain(|item| item != id);
    list.insert(0, id.to_owned());
}

/// Returned strings are owned by the caller.
#[no_mangle]
pub unsafe extern "C" fn stack_command(command: *const c_char) -> *mut c_char {
    let Ok(input) = CStr::from_ptr(command).to_str() else { return CString::default().into_raw() };
    let (action, value) = input.split_once(' ').unwrap_or((input, ""));
    let mut library = LIBRARY.lock().unwrap();
    match action {
        "push" => upsert(&mut library.history, value),
        "save" => upsert(&mut library.favorites, value),
        "remove" => {
            let ids: HashSet<_> = value.split(',').collect();
            library.history.retain(|id| !ids.contains(id.as_str()));
            library.discard_unused_search_text();
        }
        "unsave" => {
            library.favorites.retain(|id| id != value);
            library.discard_unused_search_text();
        }
        "clear" => {
            library.history.clear();
            library.discard_unused_search_text();
        }
        "history" => return CString::new(library.history.join(",")).unwrap().into_raw(),
        "favorites" => return CString::new(library.favorites.join(",")).unwrap().into_raw(),
        "search" => return CString::new(library.matches(value)).unwrap().into_raw(),
        _ => {}
    }
    CString::default().into_raw()
}

/// The text is normalized by the caller and indexed only in memory.
#[no_mangle]
pub unsafe extern "C" fn stack_index_text(id: *const c_char, text: *const u8, length: usize) {
    if length == 0 {
        return;
    }
    let Ok(id) = CStr::from_ptr(id).to_str() else { return };
    let Ok(text) = str::from_utf8(slice::from_raw_parts(text, length)) else { return };
    LIBRARY.lock().unwrap().search_text.insert(id.to_owned(), text.to_owned());
}

#[no_mangle]
pub unsafe extern "C" fn stack_free(value: *mut c_char) {
    if !value.is_null() { drop(CString::from_raw(value)); }
}

/// Returns zero on success, -1 if held by another instance, or an I/O error code.
/// A successful call transfers ownership of the locked file to the caller.
#[no_mangle]
pub unsafe extern "C" fn stack_lock_session(path: *const c_char, lock: *mut *mut File) -> c_int {
    let path = OsStr::from_bytes(CStr::from_ptr(path).to_bytes());
    let file = match OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
    {
        Ok(file) => file,
        Err(error) => return error.raw_os_error().unwrap_or(-2),
    };
    match file.try_lock() {
        Ok(()) => {
            *lock = Box::into_raw(Box::new(file));
            0
        }
        Err(TryLockError::WouldBlock) => -1,
        Err(TryLockError::Error(error)) => error.raw_os_error().unwrap_or(-2),
    }
}

#[no_mangle]
pub unsafe extern "C" fn stack_unlock_session(lock: *mut File) {
    if !lock.is_null() {
        drop(Box::from_raw(lock));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(input: &str) -> String {
        let command = CString::new(input).unwrap();
        unsafe {
            let result = stack_command(command.as_ptr());
            let output = CStr::from_ptr(result).to_str().unwrap().to_owned();
            stack_free(result);
            output
        }
    }

    #[test]
    fn history_and_favorites_have_independent_lifetimes() {
        run("push a");
        run("push b");
        run("push a");
        assert_eq!(run("history"), "a,b");
        run("save a");
        run("remove b");
        assert_eq!(run("history"), "a");
        run("clear");
        assert_eq!(run("history"), "");
        assert_eq!(run("favorites"), "a");
        run("unsave a");
        assert_eq!(run("favorites"), "");
    }

    #[test]
    fn text_search_keeps_scope_order_and_favorite_only_entries() {
        let mut library = Library {
            history: vec!["recent".into(), "shared".into(), "unrelated".into()],
            favorites: vec!["shared".into(), "saved".into()],
            search_text: HashMap::from([
                ("recent".into(), "剪栈 新版发布".into()),
                ("shared".into(), "剪栈 发布流程".into()),
                ("saved".into(), "剪栈 收藏 发布".into()),
                ("unrelated".into(), "剪栈 会议".into()),
            ]),
        };
        assert_eq!(library.matches("剪栈 发布"), "recent,shared\nshared,saved");
        library.history.clear();
        library.discard_unused_search_text();
        assert_eq!(library.matches("发布"), "\nshared,saved");
        assert_eq!(library.search_text.len(), 2);
    }
}
