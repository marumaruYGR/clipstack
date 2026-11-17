use std::{
    collections::HashSet,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tauri_plugin_global_shortcut::Shortcut;
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

use crate::{
    clipboard::Content,
    storage::{self, Storage},
};

#[derive(Clone, Copy, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Material {
    Mica,
    Acrylic,
}

#[derive(Clone, Copy, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorTheme {
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PasteMode {
    #[default]
    Direct,
    Manual,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub material: Material,
    pub theme: ColorTheme,
    pub autostart: bool,
    pub shortcut: Option<String>,
    pub notifications: bool,
    pub paste_mode: PasteMode,
    pub seen_direct_paste: bool,
    pub seen_manual_paste: bool,
    pub seen_welcome_1_1: bool,
    pub favorites_directory: PathBuf,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            material: Material::Mica,
            theme: ColorTheme::System,
            autostart: true,
            shortcut: Some("Alt+V".into()),
            notifications: true,
            paste_mode: PasteMode::Direct,
            seen_direct_paste: false,
            seen_manual_paste: false,
            seen_welcome_1_1: false,
            favorites_directory: PathBuf::new(),
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SettingsPatch {
    pub material: Option<Material>,
    pub theme: Option<ColorTheme>,
    pub autostart: Option<bool>,
    #[serde(deserialize_with = "present_shortcut")]
    pub shortcut: Option<Option<String>>,
    pub notifications: Option<bool>,
    pub paste_mode: Option<PasteMode>,
}

fn present_shortcut<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error> {
    Option::deserialize(deserializer).map(Some)
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Entry {
    pub id: String,
    pub created_at: u64,
    pub content: Content,
    #[serde(skip)]
    search_text: String,
}

impl Entry {
    pub fn new(content: Content) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            search_text: index_text(&content),
            content,
            created_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        }
    }

    fn view(&self, favorite: bool) -> ItemView {
        let (kind, text, image_file) = match &self.content {
            Content::Text { text } => (
                if is_link(text) { "link" } else { "text" },
                preview(text),
                false,
            ),
            Content::Link { text } => ("link", preview(text), false),
            Content::RichText { text, .. } => (
                "rich_text",
                preview(if text.is_empty() { "富文本" } else { text }),
                false,
            ),
            Content::Image { width, height, .. } => ("image", format!("{width} × {height}"), false),
            Content::Files { paths, .. } => (
                "files",
                preview(
                    &paths
                        .iter()
                        .map(|path| {
                            std::path::Path::new(path)
                                .file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or(path)
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
                paths.iter().any(|path| crate::preview::is_image_file(path)),
            ),
        };
        ItemView {
            id: self.id.clone(),
            created_at: self.created_at,
            kind,
            text,
            favorite,
            image_file,
        }
    }
}

fn is_link(text: &str) -> bool {
    tauri::Url::parse(text.trim())
        .is_ok_and(|url| matches!(url.scheme(), "http" | "https" | "mailto"))
}

fn normalized(text: &str) -> String {
    text.nfkd()
        .filter(|character| !is_combining_mark(*character))
        .flat_map(char::to_lowercase)
        .collect()
}

fn index_text(content: &Content) -> String {
    normalized(&crate::preview::searchable_text(content))
}

fn index_entries(result: Result<Vec<Entry>, String>) -> Result<Vec<Entry>, String> {
    result.map(|mut entries| {
        for item in &mut entries {
            item.search_text = index_text(&item.content);
        }
        entries
    })
}

fn merge_favorites(current: &mut Vec<Entry>, records: &[Entry]) -> usize {
    let mut known: HashSet<_> = current.iter().map(|entry| &entry.content).collect();
    let additions: Vec<_> = records
        .iter()
        .filter_map(|entry| {
            if !known.insert(&entry.content) {
                return None;
            }
            let mut imported = entry.clone();
            imported.id = uuid::Uuid::new_v4().to_string();
            imported.search_text = index_text(&imported.content);
            Some(imported)
        })
        .collect();
    let added = additions.len();
    current.extend(additions);
    added
}

fn preview(text: &str) -> String {
    let mut chars = text.chars();
    let mut result: String = chars.by_ref().take(1024).collect();
    if chars.next().is_some() {
        result.push('…');
    }
    result
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ItemView {
    id: String,
    created_at: u64,
    kind: &'static str,
    text: String,
    favorite: bool,
    image_file: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppView {
    history: Vec<ItemView>,
    favorites: Vec<ItemView>,
    settings: Settings,
    pinned: bool,
    notice: Option<&'static str>,
    shortcut_error: Option<String>,
    storage_error: Option<String>,
    native_backdrop: bool,
}

pub struct Model {
    pub storage: Storage,
    pub settings: Settings,
    history: Vec<Entry>,
    favorites: Vec<Entry>,
    pub pinned: bool,
    pub notice: Option<&'static str>,
    pub notice_generation: u64,
    pub pending_manual_paste: Option<u32>,
    pub registered: Option<Shortcut>,
    pub shortcut_error: Option<String>,
    pub native_backdrop: bool,
    pub preview_id: Option<String>,
    history_error: Option<String>,
    favorites_error: Option<String>,
}

#[derive(Serialize)]
pub struct RestoreResult {
    added: usize,
    skipped: usize,
    directory: Option<PathBuf>,
}

impl Model {
    pub fn open() -> Result<Self, String> {
        let storage = Storage::open()?;
        let mut settings: Settings = storage.load_settings()?;
        if settings.favorites_directory.as_os_str().is_empty() {
            settings.favorites_directory = storage::default_favorites_dir()?;
        }
        let history = index_entries(storage.load_history());
        let favorites = index_entries(storage.load_favorites(&settings.favorites_directory));
        Ok(Self {
            storage,
            settings,
            pinned: false,
            notice: None,
            notice_generation: 0,
            pending_manual_paste: None,
            registered: None,
            shortcut_error: None,
            native_backdrop: true,
            preview_id: None,
            history_error: history.as_ref().err().cloned(),
            favorites_error: favorites.as_ref().err().cloned(),
            history: history.unwrap_or_default(),
            favorites: favorites.unwrap_or_default(),
        })
    }

    pub fn view(&self, query: &str) -> AppView {
        let query = normalized(query);
        let terms: Vec<_> = query.split_whitespace().collect();
        let matches = |item: &&Entry| terms.iter().all(|term| item.search_text.contains(term));
        AppView {
            history: self
                .history
                .iter()
                .filter(matches)
                .map(|item| item.view(self.favorite_index(item).is_some()))
                .collect(),
            favorites: self
                .favorites
                .iter()
                .filter(matches)
                .map(|item| item.view(true))
                .collect(),
            settings: self.settings.clone(),
            pinned: self.pinned,
            notice: self.notice,
            shortcut_error: self.shortcut_error.clone(),
            native_backdrop: self.native_backdrop,
            storage_error: self
                .favorites_error
                .clone()
                .or_else(|| self.history_error.clone()),
        }
    }

    pub fn get(&self, id: &str) -> Result<&Entry, String> {
        self.history
            .iter()
            .chain(&self.favorites)
            .find(|item| item.id == id)
            .ok_or_else(|| "这条记录已经不存在。".into())
    }

    fn favorite_index(&self, entry: &Entry) -> Option<usize> {
        self.favorites
            .iter()
            .position(|favorite| favorite.id == entry.id)
            .or_else(|| {
                self.favorites
                    .iter()
                    .position(|favorite| favorite.content == entry.content)
            })
    }

    pub fn add(&mut self, content: Content) -> Result<(), String> {
        let existing = self
            .history
            .iter()
            .position(|entry| entry.content == content);
        let mut item = if let Some(index) = existing {
            self.history.remove(index)
        } else {
            Entry::new(content)
        };
        item.created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.history.retain(|entry| entry.content != item.content);
        self.history.insert(0, item);
        let result = self.storage.save_history(&self.history);
        self.history_error = result.as_ref().err().cloned();
        result
    }

    pub fn delete(&mut self, id: &str, from_favorites: bool) -> Result<(), String> {
        if from_favorites {
            self.ensure_favorites_readable()?;
            let Some(index) = self.favorites.iter().position(|item| item.id == id) else {
                return Ok(());
            };
            let item = self.favorites.remove(index);
            if let Err(error) = self
                .storage
                .save_favorites(&self.settings.favorites_directory, &self.favorites)
            {
                self.favorites.insert(index, item);
                return Err(error);
            }
        } else {
            return self.delete_history([id]);
        }
        Ok(())
    }

    pub fn delete_history<'a>(
        &mut self,
        ids: impl IntoIterator<Item = &'a str>,
    ) -> Result<(), String> {
        let ids: HashSet<&str> = ids.into_iter().collect();
        let retained: Vec<_> = self
            .history
            .iter()
            .filter(|item| !ids.contains(item.id.as_str()))
            .collect();
        if retained.len() != self.history.len() {
            self.storage.save_history(&retained)?;
            self.history.retain(|item| !ids.contains(item.id.as_str()));
            self.history_error = None;
        }
        Ok(())
    }

    pub fn clear(&mut self) -> Result<(), String> {
        self.storage.save_history(&Vec::<Entry>::new())?;
        self.history.clear();
        self.history_error = None;
        Ok(())
    }

    pub fn toggle_favorite(&mut self, id: &str) -> Result<(), String> {
        self.ensure_favorites_readable()?;
        let item = self.get(id)?;
        if let Some(index) = self.favorite_index(item) {
            let favorite_id = self.favorites[index].id.clone();
            return self.delete(&favorite_id, true);
        }
        self.favorites.insert(0, item.clone());
        if let Err(error) = self
            .storage
            .save_favorites(&self.settings.favorites_directory, &self.favorites)
        {
            self.favorites.remove(0);
            return Err(error);
        }
        Ok(())
    }

    pub fn move_favorites(&mut self, directory: PathBuf) -> Result<(), String> {
        self.ensure_favorites_readable()?;
        if !directory.is_absolute() {
            return Err("请输入 Windows 绝对路径。".into());
        }
        let mut next = self.settings.clone();
        next.favorites_directory = directory;
        self.storage.move_favorites(
            &self.settings.favorites_directory,
            &next.favorites_directory,
            &next,
        )?;
        self.settings = next;
        Ok(())
    }

    pub fn favorites_for_backup(&self) -> Result<Vec<Entry>, String> {
        self.ensure_favorites_readable()?;
        Ok(self.favorites.clone())
    }

    pub fn restore_favorites(&mut self, records: &[Entry]) -> Result<RestoreResult, String> {
        let current = index_entries(
            self.storage
                .load_favorites(&self.settings.favorites_directory),
        );
        let needs_new_directory = current.is_err();
        let mut merged = current.unwrap_or_else(|_| self.favorites.clone());
        let added = merge_favorites(&mut merged, records);
        let directory = if needs_new_directory {
            let mut settings = self.settings.clone();
            settings.favorites_directory = storage::default_favorites_dir()?
                .join(format!("恢复的收藏-{}", uuid::Uuid::new_v4()));
            self.storage.save_recovered_favorites(
                &settings.favorites_directory,
                &merged,
                &settings,
            )?;
            let directory = settings.favorites_directory.clone();
            self.settings = settings;
            Some(directory)
        } else {
            if added != 0 {
                self.storage
                    .save_favorites(&self.settings.favorites_directory, &merged)?;
            }
            None
        };
        self.favorites = merged;
        self.favorites_error = None;
        Ok(RestoreResult {
            added,
            skipped: records.len() - added,
            directory,
        })
    }

    fn ensure_favorites_readable(&self) -> Result<(), String> {
        match &self.favorites_error {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_preserves_local_entries_and_deduplicates_content() {
        let existing = Entry::new(Content::Text {
            text: "已有收藏".into(),
        });
        let mut copied = existing.clone();
        copied.id = uuid::Uuid::new_v4().to_string();
        let imported = Entry::new(Content::Text {
            text: "重装迁移".into(),
        });
        let records = [copied, imported.clone(), imported.clone()];
        let mut local = vec![existing.clone()];

        assert_eq!(merge_favorites(&mut local, &records), 1);
        assert_eq!(local[0].id, existing.id);
        assert_ne!(local[1].id, imported.id);
        assert_eq!(local[1].created_at, imported.created_at);
        assert_eq!(local[1].search_text, "重装迁移");
        assert_eq!(merge_favorites(&mut local, &records), 0);
        assert_eq!(local.len(), 2);
    }
}
