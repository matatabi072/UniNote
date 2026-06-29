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

/// 関連ツールの情報（プラグイン）
#[derive(Clone)]
pub struct ToolInfo {
    pub path: PathBuf,
    pub key: String,           // "simplecalendar" など内部識別子
    pub display_name: String,  // UI 表示用ラベル
    pub icon: &'static str,    // 絵文字アイコン
    /// SimpleCalendar の場合 `--linked-tasks <tasks.json>` を渡す等
    pub link_tasks_arg: bool,
}

/// 関連ツール格納フォルダ（UniNote.exe と同階層の tools/）
fn tools_dir() -> PathBuf {
    let d = data_dir().join("tools");
    let _ = fs::create_dir_all(&d);
    d
}

/// 既知ツールテーブル: (filename_lower, display_name, icon, link_tasks_arg)
fn known_tools() -> &'static [(&'static str, &'static str, &'static str, bool)] {
    // pc-reminder.exe (CLI) は本体から起動する用途がないため、リストには含めない
    // （存在する場合は GUI から内部的に呼び出される）。
    &[
        ("simplecalendar.exe", "SimpleCalendar — カレンダー表示", "📅", true),
        ("pc-reminder-gui.exe", "PCReminder — リマインダー管理", "⏰", false),
    ]
}

/// 自動検出する標準的な探索フォルダのリスト
/// - Program Files / Program Files (x86)
/// - Downloads / Desktop / Documents
/// - C:\tools / C:\tool / C:\Tools / C:\Tool
/// - UniNote.exe を置いているフォルダの親
fn well_known_search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    // Program Files (x64 / x86)
    for env in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
        if let Ok(p) = std::env::var(env) {
            let p = PathBuf::from(p);
            if p.is_dir() && !dirs.contains(&p) {
                dirs.push(p);
            }
        }
    }
    // ユーザーフォルダ系
    if let Ok(home) = std::env::var("USERPROFILE") {
        for sub in ["Downloads", "Desktop", "Documents"] {
            let p = PathBuf::from(&home).join(sub);
            if p.is_dir() {
                dirs.push(p);
            }
        }
    }
    // C:\tools 系（ケース違いも一通り）
    for name in ["tools", "tool", "Tools", "Tool"] {
        let p = PathBuf::from(format!("C:\\{name}"));
        if p.is_dir() && !dirs.contains(&p) {
            dirs.push(p);
        }
    }
    // UniNote の親フォルダ（兄弟フォルダに置いてあるケース）
    if let Some(parent) = data_dir().parent() {
        let p = parent.to_path_buf();
        if p.is_dir() && !dirs.contains(&p) {
            dirs.push(p);
        }
    }
    dirs
}

/// 指定ディレクトリ以下（深さ max_depth まで）で、指定ファイル名 (小文字比較) を探す。
/// 最初に見つかった絶対パスを返す。
fn search_for_file(root: &Path, target_lower: &str, max_depth: usize) -> Option<PathBuf> {
    let entries = fs::read_dir(root).ok()?;
    let mut subdirs: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            if let Some(name) = path.file_name() {
                if name.to_string_lossy().to_ascii_lowercase() == target_lower {
                    return Some(path);
                }
            }
        } else if path.is_dir() {
            subdirs.push(path);
        }
    }
    if max_depth > 0 {
        for sub in subdirs {
            if let Some(found) = search_for_file(&sub, target_lower, max_depth - 1) {
                return Some(found);
            }
        }
    }
    None
}

/// 関連ツール（プラグイン）を検出。
/// 検出優先順:
///   1. manual_paths で指定された絶対パス（存在チェック付き）
///   2. UniNote.exe 同階層の tools/ 直下
///   3. well_known_search_dirs() の各フォルダを深さ1まで再帰探索
pub fn discover_tools(
    manual_paths: &std::collections::BTreeMap<String, String>,
) -> Vec<ToolInfo> {
    let known = known_tools();
    let local_tools = tools_dir();
    let search_dirs = well_known_search_dirs();

    let mut out: Vec<ToolInfo> = Vec::new();
    for (fname, label, icon, link) in known {
        let key = fname.trim_end_matches(".exe").to_string();
        let mut found: Option<PathBuf> = None;

        // 1. 手動登録パス
        if let Some(p) = manual_paths.get(&key) {
            let path = PathBuf::from(p);
            if path.is_file() {
                found = Some(path);
            }
        }
        // 2. UniNote/tools/<filename>
        if found.is_none() {
            let p = local_tools.join(fname);
            if p.is_file() {
                found = Some(p);
            }
        }
        // 3. 自動検出（標準的な配置場所を深さ1まで探索）
        if found.is_none() {
            for dir in &search_dirs {
                if let Some(p) = search_for_file(dir, fname, 1) {
                    found = Some(p);
                    break;
                }
            }
        }

        if let Some(path) = found {
            out.push(ToolInfo {
                path,
                key,
                display_name: label.to_string(),
                icon,
                link_tasks_arg: *link,
            });
        }
    }
    out
}

/// 既知ツールテーブルを (key, display_name, expected_filename) のタプル列で返す。
/// UI で「未検出のツール」をリスト表示する用途。
pub fn known_tool_entries() -> Vec<(String, String, String)> {
    known_tools()
        .iter()
        .map(|(fname, label, _icon, _link)| {
            (
                fname.trim_end_matches(".exe").to_string(),
                label.to_string(),
                fname.to_string(),
            )
        })
        .collect()
}

/// UniNote の tasks.json の絶対パス
pub fn tasks_path_abs() -> PathBuf {
    tasks_path()
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
