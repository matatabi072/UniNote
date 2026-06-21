use crate::model::Item;
use crate::settings::Settings;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

fn data_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 画像格納フォルダ（存在しなければ作成）
fn images_dir() -> PathBuf {
    let d = data_dir().join("images");
    let _ = fs::create_dir_all(&d);
    d
}

/// 対応する画像拡張子か（小文字比較）
fn is_supported_image_ext(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp"
    )
}

/// 画像ファイルを images/{uuid}.{ext} にコピーし、(相対パス, 元ファイル名) を返す
pub fn import_image(src: &Path) -> Option<(String, String)> {
    let ext = src.extension()?.to_string_lossy().to_string();
    if !is_supported_image_ext(&ext) {
        return None;
    }
    let id = uuid::Uuid::new_v4().to_string();
    let dst_name = format!("{id}.{}", ext.to_ascii_lowercase());
    let dst = images_dir().join(&dst_name);
    fs::copy(src, &dst).ok()?;
    let original = src
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| dst_name.clone());
    Some((format!("images/{dst_name}"), original))
}

/// 画像メモ削除時に実体ファイルを削除する
pub fn delete_image(rel_path: &str) {
    if rel_path.starts_with("images/") {
        let _ = fs::remove_file(data_dir().join(rel_path));
    }
}

/// 相対パスから絶対パスを得る（画像表示用）
pub fn image_abs_path(rel_path: &str) -> PathBuf {
    data_dir().join(rel_path)
}

/// サイドカー格納フォルダ（存在しなければ作成）
pub fn sync_dir() -> PathBuf {
    let d = data_dir().join("sync");
    let _ = fs::create_dir_all(&d);
    d
}

/// 検出されたサイドカーアダプターの情報
#[derive(Clone)]
pub struct SidecarInfo {
    /// exe の絶対パス
    pub path: PathBuf,
    /// 表示名（例: "todoist"）
    pub display_name: String,
}

/// sync/uninote-sync-*.exe を列挙する（大文字小文字非区別）
pub fn discover_sidecars() -> Vec<SidecarInfo> {
    let dir = sync_dir();
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(&dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        let Some(name_os) = p.file_name() else {
            continue;
        };
        let name = name_os.to_string_lossy().to_string();
        let lower = name.to_ascii_lowercase();
        if !lower.starts_with("uninote-sync-") || !lower.ends_with(".exe") {
            continue;
        }
        // "uninote-sync-todoist.exe" → "todoist"
        let display = lower
            .trim_start_matches("uninote-sync-")
            .trim_end_matches(".exe")
            .to_string();
        if display.is_empty() {
            continue;
        }
        out.push(SidecarInfo {
            path: p,
            display_name: display,
        });
    }
    out.sort_by(|a, b| a.display_name.cmp(&b.display_name));
    out
}

/// データフォルダの絶対パス（サイドカー起動の作業ディレクトリ用）
pub fn data_dir_path() -> PathBuf {
    data_dir()
}

fn tasks_path() -> PathBuf {
    data_dir().join("tasks.json")
}
fn notes_path() -> PathBuf {
    data_dir().join("notes.json")
}
fn settings_path() -> PathBuf {
    data_dir().join("settings.json")
}

pub fn tasks_mtime() -> Option<SystemTime> {
    fs::metadata(tasks_path()).ok().and_then(|m| m.modified().ok())
}

pub fn notes_mtime() -> Option<SystemTime> {
    fs::metadata(notes_path()).ok().and_then(|m| m.modified().ok())
}

fn load_items(
    path: &PathBuf,
    backup_name: &str,
    corrupt_name: &str,
) -> (Vec<Item>, Option<String>) {
    let raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return (Vec::new(), None),
    };
    match serde_json::from_str::<Vec<Item>>(&raw) {
        Ok(v) => (v, None),
        Err(e) => {
            let backup = data_dir().join(backup_name);
            if let Ok(bs) = fs::read_to_string(&backup) {
                if let Ok(v) = serde_json::from_str::<Vec<Item>>(&bs) {
                    let _ = fs::copy(path, data_dir().join(corrupt_name));
                    let name = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    return (
                        v,
                        Some(format!(
                            "{name} が破損していたためバックアップから復元しました。"
                        )),
                    );
                }
            }
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            (
                Vec::new(),
                Some(format!("{name} を読み込めませんでした（新規作成します）: {e}")),
            )
        }
    }
}

fn save_items(items: &[Item], path: &PathBuf, backup_name: &str, tmp_name: &str) {
    let json = match serde_json::to_string_pretty(items) {
        Ok(j) => j,
        Err(_) => return,
    };
    let tmp = data_dir().join(tmp_name);
    if fs::write(&tmp, &json).is_err() {
        return;
    }
    if path.exists() {
        let _ = fs::copy(path, data_dir().join(backup_name));
    }
    let _ = fs::rename(&tmp, path);
}

pub fn load_tasks() -> (Vec<Item>, Option<String>) {
    load_items(
        &tasks_path(),
        "tasks.backup.json",
        "tasks.corrupt.json",
    )
}

pub fn save_tasks(items: &[Item]) {
    save_items(&items, &tasks_path(), "tasks.backup.json", "tasks.json.tmp");
}

pub fn load_notes() -> (Vec<Item>, Option<String>) {
    load_items(
        &notes_path(),
        "notes.backup.json",
        "notes.corrupt.json",
    )
}

pub fn save_notes(items: &[Item]) {
    save_items(&items, &notes_path(), "notes.backup.json", "notes.json.tmp");
}

pub fn load_settings() -> Settings {
    match fs::read_to_string(settings_path()) {
        Ok(s) => serde_json::from_str::<Settings>(&s).unwrap_or_default(),
        Err(_) => Settings::default(),
    }
}

pub fn save_settings(settings: &Settings) {
    if let Ok(json) = serde_json::to_string_pretty(settings) {
        let _ = fs::write(settings_path(), json);
    }
}
