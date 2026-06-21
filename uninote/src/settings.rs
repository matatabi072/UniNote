use serde::{Deserialize, Serialize};

/// テーマ設定
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    System,
    Dark,
    Light,
}

impl ThemeMode {
    pub fn label(self) -> &'static str {
        match self {
            ThemeMode::System => "OSに追従",
            ThemeMode::Dark => "ダーク",
            ThemeMode::Light => "ライト",
        }
    }
}

/// タスクの並び替え軸
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortMode {
    Manual,
    DateTime,
    Priority,
}

impl SortMode {
    pub fn label(self) -> &'static str {
        match self {
            SortMode::Manual => "手動",
            SortMode::DateTime => "日時順",
            SortMode::Priority => "重要度順",
        }
    }
}

/// 24時間以上先の表示形式
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum FarFormat {
    Date,
    RelativeDays,
}

/// 24時間未満の表示形式
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum NearFormat {
    Time,
    RelativeHours,
}

/// 画像メモの一覧表示形式
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum ImageDisplay {
    /// ファイル名のみ
    #[default]
    FileName,
    /// サムネイル
    Thumbnail,
}

impl ImageDisplay {
    pub fn label(self) -> &'static str {
        match self {
            ImageDisplay::FileName => "ファイル名",
            ImageDisplay::Thumbnail => "サムネイル",
        }
    }
}

/// サムネイルの形状処理
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThumbShape {
    /// 中央クロップ（縦横比保持、上下/左右を切る）
    #[default]
    Crop,
    /// ストレッチ（縦横比を変更して枠に合わせる）
    Stretch,
}

impl ThumbShape {
    pub fn label(self) -> &'static str {
        match self {
            ThumbShape::Crop => "クロップ",
            ThumbShape::Stretch => "ストレッチ",
        }
    }
}

/// 重要度ごとの背景色（sRGBA u8×4）
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct PriorityColors {
    pub high: [u8; 4],
    pub medium: [u8; 4],
    pub low: [u8; 4],
    pub none: [u8; 4],
}

impl Default for PriorityColors {
    fn default() -> Self {
        Self {
            high: [120, 40, 40, 110],
            medium: [120, 100, 40, 100],
            low: [40, 70, 110, 90],
            none: [0, 0, 0, 0],
        }
    }
}

/// 統合設定（SimpleTask + SimpleNote の全設定を統合）
#[derive(Serialize, Deserialize, Clone)]
pub struct Settings {
    pub font_family: String,
    pub font_size: f32,
    #[serde(default)]
    pub theme: ThemeMode,
    #[serde(default)]
    pub always_on_top: bool,
    pub sort_mode: SortMode,
    pub colors: PriorityColors,
    pub far_format: FarFormat,
    pub near_format: NearFormat,
    #[serde(default)]
    pub window_pos: Option<[f32; 2]>,
    #[serde(default)]
    pub window_size: Option<[f32; 2]>,
    /// メモ編集ウィンドウのサイズ
    #[serde(default)]
    pub editor_size: Option<[f32; 2]>,
    /// メモ編集ウィンドウの位置
    #[serde(default)]
    pub editor_pos: Option<[f32; 2]>,
    /// 画像メモの一覧表示形式
    #[serde(default)]
    pub image_display: ImageDisplay,
    /// サムネイル形状
    #[serde(default)]
    pub thumb_shape: ThumbShape,
    /// サムネイルの高さ（px）
    #[serde(default = "default_thumb_height")]
    pub thumb_height: f32,
    /// 画像ビューアウィンドウのサイズ
    #[serde(default)]
    pub image_viewer_size: Option<[f32; 2]>,
    /// 画像ビューアウィンドウの位置
    #[serde(default)]
    pub image_viewer_pos: Option<[f32; 2]>,
    /// 設定ウィンドウのサイズ
    #[serde(default)]
    pub settings_size: Option<[f32; 2]>,
    /// 設定ウィンドウの位置（メインウィンドウ左上からのオフセット [dx, dy]）
    #[serde(default)]
    pub settings_offset: Option<[f32; 2]>,
}

fn default_thumb_height() -> f32 {
    48.0
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            font_family: "Yu Gothic".to_string(),
            font_size: 16.0,
            theme: ThemeMode::System,
            always_on_top: false,
            sort_mode: SortMode::Manual,
            colors: PriorityColors::default(),
            far_format: FarFormat::Date,
            near_format: NearFormat::Time,
            window_pos: None,
            window_size: None,
            editor_size: None,
            editor_pos: None,
            image_display: ImageDisplay::FileName,
            thumb_shape: ThumbShape::Crop,
            thumb_height: default_thumb_height(),
            image_viewer_size: None,
            image_viewer_pos: None,
            settings_size: None,
            settings_offset: None,
        }
    }
}

pub fn font_catalog() -> Vec<(&'static str, &'static str)> {
    vec![
        ("Yu Gothic", r"C:\Windows\Fonts\YuGothR.ttc"),
        ("Meiryo", r"C:\Windows\Fonts\meiryo.ttc"),
        ("MS Gothic", r"C:\Windows\Fonts\msgothic.ttc"),
        ("MS Mincho", r"C:\Windows\Fonts\msmincho.ttc"),
        ("BIZ UDGothic", r"C:\Windows\Fonts\BIZ-UDGothicR.ttc"),
        ("BIZ UDMincho", r"C:\Windows\Fonts\BIZ-UDMinchoM.ttc"),
    ]
}

pub fn available_fonts() -> Vec<(&'static str, &'static str)> {
    font_catalog()
        .into_iter()
        .filter(|(_, path)| std::path::Path::new(path).exists())
        .collect()
}

pub fn font_path_for(name: &str) -> Option<&'static str> {
    font_catalog()
        .into_iter()
        .find(|(n, _)| *n == name)
        .map(|(_, p)| p)
}
