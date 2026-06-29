use crate::model::{Item, Priority};
use crate::settings::{
    available_fonts, font_path_for, AddTaskKey, FarFormat, ImageDisplay, NearFormat, PcAction,
    Settings, SortMode, ThemeMode, ThumbShape,
};
use crate::storage;
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime};
use eframe::egui;
use egui::{Color32, FontId, RichText};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

/// サイドカーからのメッセージ
enum SyncMsg {
    Line(String),
    Done(i32),
}

/// 実行中（または完了済みで未閉鎖）のサイドカー状態
struct ActiveSync {
    name: String,
    rx: Receiver<SyncMsg>,
    started_at: Instant,
    finished_at: Option<Instant>,
    log: Vec<String>,
    exit_code: Option<i32>,
}

const AUTOSAVE_DEBOUNCE: Duration = Duration::from_millis(800);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Task,
    Note,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConflictTarget {
    Tasks,
    Notes,
}

#[derive(Clone, Copy)]
struct DateBuf {
    y: i32,
    mo: u32,
    d: u32,
    h: u32,
    mi: u32,
}

impl DateBuf {
    fn from_dt(dt: NaiveDateTime) -> Self {
        use chrono::{Datelike, Timelike};
        Self { y: dt.year(), mo: dt.month(), d: dt.day(), h: dt.hour(), mi: dt.minute() }
    }
    fn now() -> Self {
        Self::from_dt(Local::now().naive_local())
    }
    fn to_dt(self) -> Option<NaiveDateTime> {
        NaiveDate::from_ymd_opt(self.y, self.mo, self.d)
            .and_then(|d| d.and_hms_opt(self.h, self.mi, 0))
    }
    fn clamp(&mut self) {
        self.y = self.y.clamp(1970, 9999);
        self.mo = self.mo.clamp(1, 12);
        self.d = self.d.clamp(1, 31);
        self.h = self.h.min(23);
        self.mi = self.mi.min(59);
    }
}

/// 月日時分（2桁ゼロ埋め）フォーマッタ
fn fmt2(n: f64, _: std::ops::RangeInclusive<usize>) -> String {
    format!("{:02}", n as i32)
}

/// DragValue の表示幅を縮めるためのスタイル変更。
/// 必ず ui.scope で子 UI を作って適用すること（親 UI の spacing_mut だけ
/// 書き換えても、その後の ui.add に伝わらないケースがあるため）。
fn apply_compact_style(ui: &mut egui::Ui) {
    let s = ui.style_mut();
    // DragValue が内部で使う Button の左右 padding を最小化
    s.spacing.button_padding.x = 1.0;
    // 要素間の隙間も詰める
    s.spacing.item_spacing.x = 1.0;
    // widget の最小幅も縮める（DragValue/Button が下限に張り付くのを防ぐ）
    s.spacing.interact_size.x = 0.0;
}

/// 日付部（年/月/日）を描画。DragValue を自然サイズで配置することで
/// Button/Checkbox と同じ widget 高さになり、ベースラインが揃う。
/// 月/日 は custom_formatter で 2 桁固定 → 幅も安定。
fn date_part(ui: &mut egui::Ui, b: &mut DateBuf) {
    ui.scope(|ui| {
        apply_compact_style(ui);
        ui.add(egui::DragValue::new(&mut b.y).speed(1).range(1970..=9999));
        ui.label("/");
        ui.add(
            egui::DragValue::new(&mut b.mo)
                .speed(1)
                .range(1..=12)
                .custom_formatter(fmt2),
        );
        ui.label("/");
        ui.add(
            egui::DragValue::new(&mut b.d)
                .speed(1)
                .range(1..=31)
                .custom_formatter(fmt2),
        );
    });
}

/// 時刻部（時:分）を描画。同じく add で自然サイズ + 2 桁固定。
/// 時:分 は同じ ui.horizontal の中で連続描画 → 途中で折り返されない。
fn time_part(ui: &mut egui::Ui, b: &mut DateBuf) {
    ui.scope(|ui| {
        apply_compact_style(ui);
        ui.add(
            egui::DragValue::new(&mut b.h)
                .speed(1)
                .range(0..=23)
                .custom_formatter(fmt2),
        );
        ui.label(":");
        ui.add(
            egui::DragValue::new(&mut b.mi)
                .speed(1)
                .range(0..=59)
                .custom_formatter(fmt2),
        );
    });
}

/// 年月日時分をまとめて描画（横幅に余裕がある場面用。ポップアップ等）。
fn date_fields(ui: &mut egui::Ui, b: &mut DateBuf) {
    date_part(ui, b);
    ui.add_space(4.0);
    time_part(ui, b);
    b.clamp();
}

pub struct App {
    mode: Mode,
    tasks: Vec<Item>,
    notes: Vec<Item>,
    settings: Settings,
    // タスクモード固有
    new_task_text: String,
    new_task_use_date: bool,
    new_task_buf: DateBuf,
    /// 新規タスク作成時の PC 操作種別（PCReminder への予約内容）
    new_task_pc_action: PcAction,
    editing_task_date: Option<String>,
    edit_buf: DateBuf,
    // メモモード固有
    selected_note: usize,
    editing_note: Option<String>,
    focus_editor: bool,
    /// 画像ビューアに表示中のメモID（画像メモのみ）
    viewing_image: Option<String>,
    // 共通
    show_settings: bool,
    current_font: String,
    current_theme: ThemeMode,
    current_on_top: bool,
    win_pos: Option<[f32; 2]>,
    win_size: Option<[f32; 2]>,
    dirty_tasks: bool,
    dirty_notes: bool,
    last_saved_tasks: Instant,
    last_saved_notes: Instant,
    last_tasks_mtime: Option<SystemTime>,
    last_notes_mtime: Option<SystemTime>,
    status_msg: Option<String>,
    was_focused: bool,
    external_conflict: Option<ConflictTarget>,
    monitor_size: Option<[f32; 2]>,
    geometry_checked: bool,
    // 外部連携（サイドカー）
    sidecars: Vec<storage::SidecarInfo>,
    active_sync: Option<ActiveSync>,
    show_sync_log: bool,
    // 自動同期スケジューラ
    /// 直近の自動同期スタート時刻（次回起動判定の基準）。
    /// App::new() で now を入れて、起動直後に間隔ぶん待ってから初回 tick が来るようにする。
    last_auto_sync_at: Instant,
    /// 自動同期の待機キュー（複数サイドカーを順次起動するため）
    auto_sync_queue: std::collections::VecDeque<storage::SidecarInfo>,
    /// 起動時同期を実行済みか
    startup_sync_done: bool,
    /// 現在の active_sync が自動同期由来か（自動なら終了時に静かに閉じる）
    current_sync_is_auto: bool,
    // 関連ツール（プラグイン）
    tools: Vec<storage::ToolInfo>,
    tool_status: Option<String>,
    /// PCReminder 登録/解除の通知メッセージ（バナー表示）
    reminder_status: Option<String>,
    reminder_status_is_error: bool,
    /// タスク右クリックメニュー（独立ウィンドウ）の対象タスク id
    context_menu_task_id: Option<String>,
    /// メニュー表示位置（スクリーン絶対座標 = 行の左下）
    context_menu_screen_pos: Option<[f32; 2]>,
    /// タスク内容ビューワ（ミニウィンドウ）の対象タスク id。None = 非表示。
    viewing_task_id: Option<String>,
    /// ビューワの編集モード（true = TextEdit、false = リンク化表示）
    viewing_task_edit_mode: bool,
    /// 編集モードでの編集バッファ（保存時に該当タスクに書き戻す）
    viewing_task_edit_buf: String,
    /// ホバー時に外部プレビューウィンドウへ表示するタスク本文。
    /// 各フレーム冒頭で None にリセットされ、ホバー検出時にそのタスクの本文がセットされる。
    pending_preview: Option<String>,
    // セットアップウィザード
    show_setup_wizard: bool,
    wizard_tab: WizardTab,
    wizard_todoist_token: String,
    wizard_gtasks_client_id: String,
    wizard_gtasks_client_secret: String,
    /// ウィザード実行中のサイドカーコマンドのログ（直近1回分）
    wizard_log: Vec<String>,
    wizard_status: Option<String>,
    wizard_active: Option<ActiveSync>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WizardTab {
    Todoist,
    GTasks,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let (tasks, tasks_msg) = storage::load_tasks();
        let (notes, notes_msg) = storage::load_notes();
        let last_tasks_mtime = storage::tasks_mtime();
        let last_notes_mtime = storage::notes_mtime();
        let settings = storage::load_settings();
        apply_font(&cc.egui_ctx, &settings.font_family);
        cc.egui_ctx.set_theme(theme_pref(settings.theme));
        let current_font = settings.font_family.clone();
        let current_theme = settings.theme;
        let current_on_top = settings.always_on_top;
        let pc_action_initial = settings.pc_action_default;
        // tools 初期検出（settings.manual_tool_paths を使う。move 前にスキャンしておく）
        let tools_initial = storage::discover_tools(&settings.manual_tool_paths);
        let status_msg = tasks_msg.or(notes_msg);
        Self {
            mode: Mode::Task,
            tasks,
            notes,
            settings,
            new_task_text: String::new(),
            new_task_use_date: false,
            new_task_buf: DateBuf::now(),
            // settings は move 後なので、あらかじめ取り出した初期値を使う
            new_task_pc_action: pc_action_initial,
            editing_task_date: None,
            edit_buf: DateBuf::now(),
            selected_note: 0,
            editing_note: None,
            focus_editor: false,
            viewing_image: None,
            show_settings: false,
            current_font,
            current_theme,
            current_on_top,
            win_pos: None,
            win_size: None,
            dirty_tasks: false,
            dirty_notes: false,
            last_saved_tasks: Instant::now(),
            last_saved_notes: Instant::now(),
            last_tasks_mtime,
            last_notes_mtime,
            status_msg,
            was_focused: false,
            external_conflict: None,
            monitor_size: None,
            geometry_checked: false,
            sidecars: storage::discover_sidecars(),
            active_sync: None,
            show_sync_log: false,
            last_auto_sync_at: Instant::now(),
            auto_sync_queue: std::collections::VecDeque::new(),
            startup_sync_done: false,
            current_sync_is_auto: false,
            tools: tools_initial,
            tool_status: None,
            reminder_status: None,
            reminder_status_is_error: false,
            context_menu_task_id: None,
            context_menu_screen_pos: None,
            viewing_task_id: None,
            viewing_task_edit_mode: false,
            viewing_task_edit_buf: String::new(),
            pending_preview: None,
            show_setup_wizard: false,
            wizard_tab: WizardTab::Todoist,
            wizard_todoist_token: String::new(),
            wizard_gtasks_client_id: String::new(),
            wizard_gtasks_client_secret: String::new(),
            wizard_log: Vec::new(),
            wizard_status: None,
            wizard_active: None,
        }
    }

    /// 指定サイドカー exe を実引数で実行し、stdout/stderr を wizard_active に流す
    fn run_sidecar_command(
        &mut self,
        sidecar_name: &str,
        args: Vec<String>,
        display_label: String,
    ) {
        let exe_path = match self.sidecars.iter().find(|s| s.display_name == sidecar_name) {
            Some(s) => s.path.clone(),
            None => {
                self.wizard_status = Some(format!(
                    "❌ {sidecar_name} サイドカーが sync/ フォルダにありません"
                ));
                return;
            }
        };
        let (tx, rx) = channel();
        let exe_dir = storage::data_dir_path();
        let mut cmd = Command::new(&exe_path);
        cmd.current_dir(&exe_dir).args(&args).stdout(Stdio::piped()).stderr(Stdio::piped());
        // Windows のコンソールウィンドウを抑制
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        match cmd.spawn() {
            Ok(mut child) => {
                if let Some(out) = child.stdout.take() {
                    let tx2 = tx.clone();
                    thread::spawn(move || {
                        for line in BufReader::new(out).lines().map_while(Result::ok) {
                            if tx2.send(SyncMsg::Line(line)).is_err() {
                                break;
                            }
                        }
                    });
                }
                if let Some(err) = child.stderr.take() {
                    let tx2 = tx.clone();
                    thread::spawn(move || {
                        for line in BufReader::new(err).lines().map_while(Result::ok) {
                            if tx2.send(SyncMsg::Line(format!("[err] {line}"))).is_err() {
                                break;
                            }
                        }
                    });
                }
                let tx_done = tx;
                thread::spawn(move || {
                    let mut child = child;
                    let code = child.wait().map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
                    let _ = tx_done.send(SyncMsg::Done(code));
                });
                self.wizard_active = Some(ActiveSync {
                    name: display_label,
                    rx,
                    started_at: Instant::now(),
                    finished_at: None,
                    log: Vec::new(),
                    exit_code: None,
                });
                self.wizard_status = None;
            }
            Err(e) => {
                self.wizard_status = Some(format!("❌ サイドカー起動失敗: {e}"));
            }
        }
    }

    /// PCReminder CLI exe のパス（tools/pc-reminder.exe）。なければ None。
    fn pcr_cli_path() -> Option<PathBuf> {
        let p = storage::data_dir_path().join("tools").join("pc-reminder.exe");
        if p.is_file() { Some(p) } else { None }
    }

    /// 起動時にスキャン済みの self.tools に指定 key のツールが含まれているか。
    /// 既知ツールテーブル（discover_tools）の key と一致する。
    /// 例: "pc-reminder-gui" / "simplecalendar"
    fn is_tool_available(&self, key: &str) -> bool {
        self.tools.iter().any(|t| t.key == key)
    }

    /// 指定タスクの予定日時を PCReminder に登録する。
    /// 検証: scheduledDateTime 必須、現在時刻より未来であること。失敗時はバナーで通知。
    fn register_reminder(&mut self, idx: usize) {
        self.reminder_status = None;
        let cli = match Self::pcr_cli_path() {
            Some(p) => p,
            None => {
                self.reminder_status = Some(
                    "⚠ PCReminder が tools/ に見つかりません。pc-reminder.exe を配置してください。".into(),
                );
                self.reminder_status_is_error = true;
                return;
            }
        };
        let task = match self.tasks.get(idx) {
            Some(t) => t.clone(),
            None => return,
        };
        let dt = match task.scheduled_date_time {
            Some(d) => d,
            None => {
                self.reminder_status = Some(
                    "⚠ 予定日時が設定されていません。先に予定日時を編集してから登録してください。"
                        .into(),
                );
                self.reminder_status_is_error = true;
                return;
            }
        };
        let now = chrono::Local::now().naive_local();
        if dt <= now {
            self.reminder_status = Some(format!(
                "⚠ 予定日時 ({}) が現在時刻より過去です。リマインダーは登録できません。",
                dt.format("%Y/%m/%d %H:%M")
            ));
            self.reminder_status_is_error = true;
            return;
        }

        let key = format!(
            "uninote-{}",
            &task.id[..task.id.len().min(8)]
        );
        let time_str = dt.format("%Y/%m/%d %H:%M").to_string();
        let content = if task.task_content.trim().is_empty() {
            "(無題のタスク)".to_string()
        } else {
            task.task_content.clone()
        };

        let mut cmd = Command::new(&cli);
        cmd.current_dir(storage::data_dir_path());
        cmd.arg("/add_remind")
            .arg("--key")
            .arg(&key)
            .arg(&time_str)
            .arg(&content);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        match cmd.output() {
            Ok(out) => {
                if out.status.success() {
                    self.tasks[idx].reminder_key = Some(key);
                    self.tasks[idx].touch();
                    self.dirty_tasks = true;
                    self.reminder_status = Some(format!(
                        "🔔 リマインダー登録: {} に通知します",
                        time_str
                    ));
                    self.reminder_status_is_error = false;
                } else {
                    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
                    self.reminder_status = Some(format!(
                        "❌ 登録失敗 (exit {}): {}",
                        out.status.code().unwrap_or(-1),
                        stderr.trim()
                    ));
                    self.reminder_status_is_error = true;
                }
            }
            Err(e) => {
                self.reminder_status = Some(format!("❌ pc-reminder 起動失敗: {e}"));
                self.reminder_status_is_error = true;
            }
        }
    }

    /// 指定タスクに任意の PC 操作を予約する（通知/起動/スリープ）。
    /// PcAction の値に応じて pc-reminder に渡すコマンドを切り替える。
    fn register_pc_action(&mut self, idx: usize, action: PcAction) {
        self.reminder_status = None;
        if action == PcAction::None {
            return;
        }
        let cli = match Self::pcr_cli_path() {
            Some(p) => p,
            None => {
                self.reminder_status = Some(
                    "⚠ PCReminder が tools/ に見つかりません。pc-reminder.exe を配置してください。".into(),
                );
                self.reminder_status_is_error = true;
                return;
            }
        };
        let task = match self.tasks.get(idx) {
            Some(t) => t.clone(),
            None => return,
        };
        let dt = match task.scheduled_date_time {
            Some(d) => d,
            None => {
                self.reminder_status = Some(
                    "⚠ 予定日時が設定されていません。先に予定日時を編集してください。".into(),
                );
                self.reminder_status_is_error = true;
                return;
            }
        };
        let now = chrono::Local::now().naive_local();
        if dt <= now {
            self.reminder_status = Some(format!(
                "⚠ 予定日時 ({}) が現在時刻より過去です。予約できません。",
                dt.format("%Y/%m/%d %H:%M")
            ));
            self.reminder_status_is_error = true;
            return;
        }

        let key = format!("uninote-{}", &task.id[..task.id.len().min(8)]);
        let time_str = dt.format("%Y/%m/%d %H:%M").to_string();
        let content = if task.task_content.trim().is_empty() {
            "(無題のタスク)".to_string()
        } else {
            task.task_content.clone()
        };

        // PcAction → pc-reminder CLI コマンド
        let (subcmd, with_wake, needs_message, label) = match action {
            PcAction::Notify => ("/add_remind", false, true, "通知"),
            PcAction::Wake => ("/add_remind", true, true, "起動＋通知"),
            PcAction::Sleep => ("/add_sleep", false, false, "スリープ"),
            PcAction::None => return,
        };

        let mut cmd = Command::new(&cli);
        cmd.current_dir(storage::data_dir_path());
        cmd.arg(subcmd).arg("--key").arg(&key);
        if with_wake {
            cmd.arg("--wake");
        }
        cmd.arg(&time_str);
        if needs_message {
            cmd.arg(&content);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        match cmd.output() {
            Ok(out) => {
                if out.status.success() {
                    self.tasks[idx].reminder_key = Some(key);
                    self.tasks[idx].touch();
                    self.dirty_tasks = true;
                    self.reminder_status = Some(format!(
                        "✅ {} を予約: {} に発火します",
                        label, time_str
                    ));
                    self.reminder_status_is_error = false;
                } else {
                    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
                    self.reminder_status = Some(format!(
                        "❌ {} 予約失敗 (exit {}): {}",
                        label,
                        out.status.code().unwrap_or(-1),
                        stderr.trim()
                    ));
                    self.reminder_status_is_error = true;
                }
            }
            Err(e) => {
                self.reminder_status = Some(format!("❌ pc-reminder 起動失敗: {e}"));
                self.reminder_status_is_error = true;
            }
        }
    }

    /// 指定タスクのリマインダーを解除する。
    fn unregister_reminder(&mut self, idx: usize) {
        self.reminder_status = None;
        let task = match self.tasks.get(idx) {
            Some(t) => t.clone(),
            None => return,
        };
        let key = match &task.reminder_key {
            Some(k) => k.clone(),
            None => return,
        };
        // ローカルからはまず除去（CLI 失敗でも消す: スケジューラ側に残骸があっても /remove で再試行可能）
        self.tasks[idx].reminder_key = None;
        self.tasks[idx].touch();
        self.dirty_tasks = true;

        let cli = match Self::pcr_cli_path() {
            Some(p) => p,
            None => {
                self.reminder_status = Some(
                    "🔕 ローカルでは解除しました（pc-reminder 未配置のため Windows タスクスケジューラからの削除は手動でお願いします）".into(),
                );
                self.reminder_status_is_error = true;
                return;
            }
        };
        let mut cmd = Command::new(&cli);
        cmd.current_dir(storage::data_dir_path());
        cmd.arg("/remove").arg(&key);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        match cmd.output() {
            Ok(out) => {
                if out.status.success() {
                    self.reminder_status = Some("🔕 リマインダーを解除しました".into());
                    self.reminder_status_is_error = false;
                } else {
                    self.reminder_status = Some(format!(
                        "🔕 解除済 (タスクスケジューラ側 exit {} のため再確認推奨)",
                        out.status.code().unwrap_or(-1)
                    ));
                    self.reminder_status_is_error = false;
                }
            }
            Err(e) => {
                self.reminder_status = Some(format!("🔕 解除済（pc-reminder 起動失敗: {e}）"));
                self.reminder_status_is_error = true;
            }
        }
    }

    /// 関連ツール（プラグイン）を独立プロセスとして起動。
    /// 連携対応ツールには UniNote の tasks.json パスを引数で渡す。
    fn launch_tool(&mut self, tool: &storage::ToolInfo) {
        let mut cmd = Command::new(&tool.path);
        cmd.current_dir(storage::data_dir_path());
        if tool.link_tasks_arg {
            cmd.arg("--linked-tasks").arg(storage::tasks_path_abs());
        }
        // GUI ツールはコンソール非表示
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        match cmd.spawn() {
            Ok(_child) => {
                self.tool_status = Some(format!("✅ {} を起動しました", tool.display_name));
            }
            Err(e) => {
                self.tool_status = Some(format!("❌ {} 起動失敗: {e}", tool.display_name));
            }
        }
    }

    /// メインウィンドウが常に最前面の場合、サブウィンドウも同じレベルにする。
    /// そうしないと AlwaysOnTop のメインに隠れて操作できなくなる。
    fn sub_window_level(&self) -> egui::WindowLevel {
        if self.settings.always_on_top {
            egui::WindowLevel::AlwaysOnTop
        } else {
            egui::WindowLevel::Normal
        }
    }

    /// タスク右クリックメニューを独立 OS ウィンドウとして描画する。
    /// 右クリックされた行の直下（スクリーン絶対座標）に表示。
    /// メニュー項目選択 / Esc / × / フォーカス喪失 で閉じる。
    fn draw_context_menu(&mut self, ctx: &egui::Context) {
        let Some(task_id) = self.context_menu_task_id.clone() else {
            return;
        };
        let pos = self.context_menu_screen_pos.unwrap_or([0.0, 0.0]);
        // メニュー高さ: タイトルバー + ラベル + 重要度ボタン4個 + セパレータ +
        //   予定日時編集 + リマインダー登録/解除 + 削除 を見切れずに収める
        let size = [220.0, 320.0];

        let has_reminder = self
            .tasks
            .iter()
            .find(|t| t.id == task_id)
            .map(|t| t.reminder_key.is_some())
            .unwrap_or(false);
        let pcr_available = self.is_tool_available("pc-reminder-gui");

        let mut builder = egui::ViewportBuilder::default()
            .with_title("メニュー — UniNote")
            .with_inner_size(size)
            .with_min_inner_size([180.0, 200.0])
            // 念のためリサイズ可能にして、項目が増えても拡張できるように
            .with_resizable(true)
            .with_position(pos)
            .with_window_level(self.sub_window_level());
        // モニタ範囲外にはみ出ないようクランプ
        if let Some(mon) = self.monitor_size {
            let clamped = clamp_pos(pos, size, mon);
            builder = builder.with_position(clamped);
        }

        let mut close = false;
        let mut chosen: Option<RowAction> = None;

        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("task_context_menu"),
            builder,
            |ctx2, _class| {
                egui::CentralPanel::default().show(ctx2, |ui| {
                    // 縦スクロール可能。ウィンドウが小さくなっても全項目に到達できる。
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if let Some(a) = row_menu(ui, has_reminder, pcr_available) {
                                chosen = Some(a);
                            }
                        });
                });
                // 閉じる条件: × / Esc / フォーカス喪失
                if ctx2.input(|i| i.viewport().close_requested()) {
                    close = true;
                }
                if ctx2.input(|i| i.key_pressed(egui::Key::Escape)) {
                    close = true;
                }
                if !ctx2.input(|i| i.viewport().focused.unwrap_or(true)) {
                    close = true;
                }
            },
        );

        // 選択されたアクションを実行
        if let Some(action) = chosen {
            if let Some(i) = self.tasks.iter().position(|t| t.id == task_id) {
                match action {
                    RowAction::SetPriority(p) => {
                        if let Some(t) = self.tasks.get_mut(i) {
                            t.priority = p;
                            t.touch();
                            self.dirty_tasks = true;
                        }
                    }
                    RowAction::EditDate => {
                        if let Some(t) = self.tasks.get(i) {
                            self.edit_buf = t
                                .scheduled_date_time
                                .map(DateBuf::from_dt)
                                .unwrap_or_else(DateBuf::now);
                            self.editing_task_date = Some(t.id.clone());
                        }
                    }
                    RowAction::Delete => {
                        if i < self.tasks.len() {
                            if self.tasks[i].reminder_key.is_some() {
                                self.unregister_reminder(i);
                            }
                            self.tasks.remove(i);
                            self.renumber_tasks();
                            self.dirty_tasks = true;
                        }
                    }
                    RowAction::SetReminder => {
                        self.register_reminder(i);
                    }
                    RowAction::ClearReminder => {
                        self.unregister_reminder(i);
                    }
                }
            }
            close = true;
        }

        if close {
            self.context_menu_task_id = None;
            self.context_menu_screen_pos = None;
        }
    }

    /// ホバー時にメインウィンドウの右側（収まらなければ左側）に出す
    /// タスク本文プレビュー。装飾なし・クリックスルー・常に最前面。
    /// メインウィンドウより広めの幅を取り、URL は色付け表示する（クリックは下に通る）。
    fn draw_hover_preview(&self, ctx: &egui::Context) {
        let Some(content) = self.pending_preview.clone() else {
            return;
        };
        // ビューワ/コンテキストメニュー表示中は出さない
        if self.viewing_task_id.is_some() || self.context_menu_task_id.is_some() {
            return;
        }
        let (Some(wp), Some(ws), Some(mon)) =
            (self.win_pos, self.win_size, self.monitor_size)
        else {
            return;
        };

        // 本文表示用に少し大きめ。固定サイズ + スクロール対応で内容が長くても可視。
        let pv_size = [360.0_f32, 240.0_f32];

        // X: 右側優先、収まらなければ左側、それでもダメなら片側にクランプ
        let right_x = wp[0] + ws[0] + 6.0;
        let left_x = wp[0] - pv_size[0] - 6.0;
        let x = if right_x + pv_size[0] <= mon[0] {
            right_x
        } else if left_x >= 0.0 {
            left_x
        } else {
            (mon[0] - pv_size[0]).max(0.0)
        };

        // Y: カーソルY位置（メインウィンドウのコンテンツ座標系）を中央に揃える。
        // 取得できなかった場合は窓上端から 40px の位置をフォールバックに（wp[1] は加算しない）。
        let cursor_y = ctx
            .input(|i| i.pointer.latest_pos())
            .map(|p| p.y)
            .unwrap_or(40.0);
        let y_screen = wp[1] + cursor_y - pv_size[1] / 2.0;
        let y = y_screen
            .clamp(wp[1], wp[1] + ws[1] - pv_size[1])
            .max(0.0)
            .min(mon[1] - pv_size[1]);

        let builder = egui::ViewportBuilder::default()
            .with_title("preview")
            .with_inner_size(pv_size)
            .with_min_inner_size(pv_size)
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_window_level(egui::WindowLevel::AlwaysOnTop)
            .with_position([x, y])
            // クリックスルー: マウス操作は常に下（メインウィンドウ）へ通す
            .with_mouse_passthrough(true);

        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("hover_preview"),
            builder,
            |ctx2, _class| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::popup(&ctx2.style()))
                    .show(ctx2, |ui| {
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                if content.trim().is_empty() {
                                    ui.weak("(本文なし)");
                                    return;
                                }
                                // ビューワと同じ要領で URL を色付け表示
                                // （クリックスルーなのでクリックは下に通る）
                                let segments = split_url_segments(&content);
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing.x = 0.0;
                                    for seg in segments {
                                        match seg {
                                            TextSegment::Text(t) => {
                                                let mut first = true;
                                                for line in t.split('\n') {
                                                    if !first {
                                                        ui.end_row();
                                                    }
                                                    if !line.is_empty() {
                                                        ui.label(line);
                                                    }
                                                    first = false;
                                                }
                                            }
                                            TextSegment::Url(u) => {
                                                ui.label(
                                                    RichText::new(&u)
                                                        .color(Color32::from_rgb(
                                                            100, 170, 240,
                                                        ))
                                                        .underline(),
                                                );
                                            }
                                        }
                                    }
                                });
                            });
                    });
            },
        );
    }

    /// タスク内容ビューワ（別 OS ウィンドウ）。
    /// 表示モード = URL リンク化済みテキスト + メタ情報。
    /// 編集モード = TextEdit::multiline で task_content を直接編集。
    /// ESC: 編集中ならキャンセル、表示中ならウィンドウを閉じる。
    fn draw_task_viewer(&mut self, ctx: &egui::Context) {
        let Some(task_id) = self.viewing_task_id.clone() else {
            return;
        };
        // 対象タスクのスナップショット（表示中に他で削除された場合は閉じる）
        let task = match self.tasks.iter().find(|t| t.id == task_id) {
            Some(t) => t.clone(),
            None => {
                self.viewing_task_id = None;
                self.viewing_task_edit_mode = false;
                self.viewing_task_edit_buf.clear();
                return;
            }
        };

        let size = self.settings.task_view_size.unwrap_or([480.0, 320.0]);
        let mut builder = egui::ViewportBuilder::default()
            .with_title("📝 タスク内容 — UniNote")
            .with_inner_size(size)
            .with_min_inner_size([280.0, 200.0])
            .with_resizable(true)
            .with_window_level(self.sub_window_level());

        // 初期位置: メインウィンドウ左上 + 保存オフセット（初回はメインの中央）
        let initial_pos: Option<[f32; 2]> = match (self.win_pos, self.monitor_size) {
            (Some(mp), Some(mon)) => {
                let offset = self.settings.task_view_offset.unwrap_or_else(|| {
                    let ms = self.win_size.unwrap_or([0.0, 0.0]);
                    [(ms[0] - size[0]) / 2.0, (ms[1] - size[1]) / 2.0]
                });
                let candidate = [mp[0] + offset[0], mp[1] + offset[1]];
                Some(clamp_pos(candidate, size, mon))
            }
            _ => None,
        };
        if let Some(p) = initial_pos {
            builder = builder.with_position(p);
        }

        let mut close = false;
        let mut save_content: Option<String> = None;
        // 「同じタスク作成」要求。Some(()) = 複製要求あり。
        // 処理は viewport 抜けた後に行う（self.tasks への push と viewing_task_id 差し替え）。
        let mut duplicate_request = false;

        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("task_viewer"),
            builder,
            |ctx2, _class| {
                // 位置・サイズの記憶（メインウィンドウ左上を基準にオフセット保存）
                ctx2.input(|i| {
                    let vp = i.viewport();
                    if let Some(inner) = vp.inner_rect {
                        self.settings.task_view_size =
                            Some([inner.width(), inner.height()]);
                    }
                    if let Some(outer) = vp.outer_rect {
                        if let Some(mp) = self.win_pos {
                            self.settings.task_view_offset =
                                Some([outer.min.x - mp[0], outer.min.y - mp[1]]);
                        }
                    }
                });

                // ─── 上段: メタ情報 ───
                egui::TopBottomPanel::top("tv_meta").show(ctx2, |ui| {
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        if task.is_completed {
                            ui.colored_label(
                                Color32::from_rgb(140, 200, 140),
                                "✓ 完了",
                            );
                        } else {
                            ui.label("○ 未完了");
                        }
                        ui.separator();
                        ui.label("優先度:");
                        let prio_label = match task.priority {
                            Priority::High => "高",
                            Priority::Medium => "中",
                            Priority::Low => "低",
                            Priority::None => "なし",
                        };
                        ui.label(prio_label);
                        if let Some(dt) = task.scheduled_date_time {
                            ui.separator();
                            ui.label("予定:");
                            ui.label(dt.format("%Y-%m-%d %H:%M").to_string());
                        }
                    });
                    ui.add_space(4.0);
                });

                // ─── 下段: 操作ボタン ───
                egui::TopBottomPanel::bottom("tv_buttons").show(ctx2, |ui| {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if self.viewing_task_edit_mode {
                            if ui.button("💾 保存").clicked() {
                                save_content =
                                    Some(self.viewing_task_edit_buf.clone());
                                self.viewing_task_edit_mode = false;
                            }
                            if ui.button("キャンセル").clicked() {
                                self.viewing_task_edit_buf =
                                    task.task_content.clone();
                                self.viewing_task_edit_mode = false;
                            }
                        } else {
                            if ui.button("✏ 編集").clicked() {
                                self.viewing_task_edit_buf =
                                    task.task_content.clone();
                                self.viewing_task_edit_mode = true;
                            }
                            if ui
                                .button("📋 同じタスク作成")
                                .on_hover_text(
                                    "同じ内容で新規タスクを作成し、編集モードで開きます",
                                )
                                .clicked()
                            {
                                duplicate_request = true;
                            }
                            if ui.button("閉じる").clicked() {
                                close = true;
                            }
                        }
                    });
                    ui.add_space(6.0);
                });

                // ─── 中段: 本文 ───
                egui::CentralPanel::default().show(ctx2, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if self.viewing_task_edit_mode {
                                ui.add_sized(
                                    ui.available_size(),
                                    egui::TextEdit::multiline(
                                        &mut self.viewing_task_edit_buf,
                                    )
                                    .desired_rows(8),
                                );
                            } else if task.task_content.trim().is_empty() {
                                ui.weak("(本文なし)");
                            } else {
                                // URL をリンク化して描画。クリックで rundll32 経由で
                                // 既定ブラウザを開く（cmd の & 問題回避済み）。
                                let segments =
                                    split_url_segments(&task.task_content);
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing.x = 0.0;
                                    for seg in segments {
                                        match seg {
                                            TextSegment::Text(t) => {
                                                // 改行を保つために行ごとに描画
                                                let mut first = true;
                                                for line in t.split('\n') {
                                                    if !first {
                                                        ui.end_row();
                                                    }
                                                    if !line.is_empty() {
                                                        ui.label(line);
                                                    }
                                                    first = false;
                                                }
                                            }
                                            TextSegment::Url(u) => {
                                                let resp = ui.add(
                                                    egui::Label::new(
                                                        RichText::new(&u)
                                                            .color(
                                                                Color32::from_rgb(
                                                                    100, 170,
                                                                    240,
                                                                ),
                                                            )
                                                            .underline(),
                                                    )
                                                    .sense(
                                                        egui::Sense::click(),
                                                    ),
                                                );
                                                if resp.hovered() {
                                                    ctx2.set_cursor_icon(
                                                        egui::CursorIcon::PointingHand,
                                                    );
                                                }
                                                if resp.clicked() {
                                                    open_url(&u);
                                                }
                                                resp.on_hover_text(&u);
                                            }
                                        }
                                    }
                                });
                            }
                        });
                });

                // 閉じる条件: × / ESC
                if ctx2.input(|i| i.viewport().close_requested()) {
                    close = true;
                }
                if ctx2.input(|i| i.key_pressed(egui::Key::Escape)) {
                    if self.viewing_task_edit_mode {
                        // 編集中の ESC は編集キャンセル（ウィンドウは閉じない）
                        self.viewing_task_edit_buf = task.task_content.clone();
                        self.viewing_task_edit_mode = false;
                    } else {
                        close = true;
                    }
                }
            },
        );

        // 保存要求があればタスクへ書き戻し
        if let Some(new_content) = save_content {
            if let Some(t) = self.tasks.iter_mut().find(|t| t.id == task_id) {
                if t.task_content != new_content {
                    t.task_content = new_content;
                    t.touch();
                    self.dirty_tasks = true;
                }
            }
        }

        // 複製要求: タイトル/優先度/予定日時 を引き継いだ新規タスクを作成し、
        // 既存ウィンドウを閉じて新タスクのビューワを編集モードで開く。
        if duplicate_request {
            let next_order = self
                .tasks
                .iter()
                .map(|t| t.manual_order)
                .max()
                .unwrap_or(0)
                + 1;
            let mut new_task = Item::new_task(task.task_content.clone(), next_order);
            new_task.priority = task.priority;
            new_task.scheduled_date_time = task.scheduled_date_time;
            let new_id = new_task.id.clone();
            let new_content = new_task.task_content.clone();
            self.tasks.push(new_task);
            self.dirty_tasks = true;
            // 既存ウィンドウは上書きで閉じ、新規タスクの編集モードで再オープン
            self.viewing_task_id = Some(new_id);
            self.viewing_task_edit_mode = true;
            self.viewing_task_edit_buf = new_content;
        } else if close {
            self.viewing_task_id = None;
            self.viewing_task_edit_mode = false;
            self.viewing_task_edit_buf.clear();
        }
    }

    fn draw_setup_wizard(&mut self, ctx: &egui::Context) {
        let size = [560.0, 600.0];
        let mut builder = egui::ViewportBuilder::default()
            .with_title("🔗 外部連携の設定 — UniNote")
            .with_inner_size(size)
            .with_min_inner_size([420.0, 380.0])
            .with_window_level(self.sub_window_level());
        // メインウィンドウ中央に重ねる
        if let (Some(mp), Some(ms), Some(mon)) =
            (self.win_pos, self.win_size, self.monitor_size)
        {
            let cx = mp[0] + (ms[0] - size[0]) / 2.0;
            let cy = mp[1] + (ms[1] - size[1]) / 2.0;
            builder = builder.with_position(clamp_pos([cx, cy], size, mon));
        }

        let mut close = false;
        let busy = self
            .wizard_active
            .as_ref()
            .map(|a| a.exit_code.is_none())
            .unwrap_or(false);

        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("setup_wizard"),
            builder,
            |ctx2, _class| {
                egui::TopBottomPanel::top("wiz_tabs").show(ctx2, |ui| {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui
                            .selectable_label(self.wizard_tab == WizardTab::Todoist, "Todoist")
                            .clicked()
                            && self.wizard_tab != WizardTab::Todoist
                        {
                            self.wizard_tab = WizardTab::Todoist;
                            // タブ切替時に前タブの実行ログ表示をクリア
                            if self
                                .wizard_active
                                .as_ref()
                                .map(|a| a.exit_code.is_some())
                                .unwrap_or(false)
                            {
                                self.wizard_active = None;
                            }
                            self.wizard_status = None;
                        }
                        if ui
                            .selectable_label(
                                self.wizard_tab == WizardTab::GTasks,
                                "Google Tasks",
                            )
                            .clicked()
                            && self.wizard_tab != WizardTab::GTasks
                        {
                            self.wizard_tab = WizardTab::GTasks;
                            if self
                                .wizard_active
                                .as_ref()
                                .map(|a| a.exit_code.is_some())
                                .unwrap_or(false)
                            {
                                self.wizard_active = None;
                            }
                            self.wizard_status = None;
                        }
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                // 実行中は閉じるを無効化（押しても反応しない誤解を防ぐ）
                                if ui
                                    .add_enabled(!busy, egui::Button::new("閉じる"))
                                    .clicked()
                                {
                                    close = true;
                                }
                            },
                        );
                    });
                    ui.add_space(4.0);
                });

                egui::CentralPanel::default().show(ctx2, |ui| {
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(
                        ui,
                        |ui| match self.wizard_tab {
                            WizardTab::Todoist => self.draw_wizard_todoist(ui, busy),
                            WizardTab::GTasks => self.draw_wizard_gtasks(ui, busy),
                        },
                    );
                });

                if ctx2.input(|i| i.viewport().close_requested()) {
                    close = true;
                }
            },
        );

        if close && !busy {
            self.show_setup_wizard = false;
            self.wizard_log.clear();
        }
    }

    fn draw_wizard_todoist(&mut self, ui: &mut egui::Ui, busy: bool) {
        ui.heading("Todoist セットアップ");
        ui.add_space(6.0);
        ui.label("Todoist のタスクを UniNote と同期できます（v2 双方向）。");
        ui.add_space(8.0);

        ui.label(RichText::new("手順 1: API トークンを取得").strong());
        ui.horizontal_wrapped(|ui| {
            ui.label("下のボタンで Todoist の Developer 設定ページを開き、");
            ui.label(RichText::new("「API token」").italics());
            ui.label("をコピーします。");
        });
        if ui.button("🌐 Todoist API 設定ページを開く").clicked() {
            open_url("https://app.todoist.com/app/settings/integrations/developer");
        }
        ui.add_space(10.0);

        ui.label(RichText::new("手順 2: トークンを貼り付け").strong());
        ui.horizontal(|ui| {
            ui.label("API token:");
            ui.add(
                egui::TextEdit::singleline(&mut self.wizard_todoist_token)
                    .password(true)
                    .desired_width(320.0)
                    .hint_text("ここに貼り付け（暗号化保存されます）"),
            );
        });
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            let can_save = !busy && !self.wizard_todoist_token.trim().is_empty();
            if ui
                .add_enabled(can_save, egui::Button::new("💾 保存（DPAPI 暗号化）"))
                .clicked()
            {
                let tok = self.wizard_todoist_token.trim().to_string();
                self.wizard_todoist_token.clear();
                self.run_sidecar_command(
                    "todoist",
                    vec!["--set-token".into(), tok],
                    "Todoist トークン保存".into(),
                );
            }
            if ui
                .add_enabled(!busy, egui::Button::new("🔍 現在の状態を確認"))
                .clicked()
            {
                self.run_sidecar_command(
                    "todoist",
                    vec!["--status".into()],
                    "Todoist 状態確認".into(),
                );
            }
            if ui
                .add_enabled(!busy, egui::Button::new("🗑 トークン削除"))
                .clicked()
            {
                self.run_sidecar_command(
                    "todoist",
                    vec!["--clear-token".into()],
                    "Todoist トークン削除".into(),
                );
            }
        });

        ui.add_space(12.0);
        self.draw_wizard_status_and_log(ui);
    }

    fn draw_wizard_gtasks(&mut self, ui: &mut egui::Ui, busy: bool) {
        ui.heading("Google Tasks セットアップ");
        ui.add_space(6.0);
        ui.label(
            "Google Tasks のタスクを UniNote へ取り込めます（v1: PULL のみ）。",
        );
        ui.label(
            "OAuth 2.0 認証が必要なので、Google Cloud Console で OAuth クライアントを作成します。",
        );
        ui.add_space(8.0);

        ui.label(RichText::new("手順 1: Google Cloud Console でセットアップ").strong());
        ui.label("下のボタンで Cloud Console を開き、以下を順に行ってください:");
        ui.indent("g_setup", |ui| {
            ui.label("①「APIとサービス → ライブラリ」で “Google Tasks API” を有効化");
            ui.label("②「APIとサービス → OAuth同意画面」→ 対象 “外部” → 自分の Gmail をテストユーザーに追加");
            ui.label("③「APIとサービス → 認証情報」→ 「+ 認証情報を作成 → OAuth クライアント ID」");
            ui.label("④ アプリケーションの種類: “デスクトップアプリ” で作成");
            ui.label("⑤ 表示された「クライアントID」と「クライアントシークレット」をコピー");
        });
        ui.horizontal(|ui| {
            if ui.button("🌐 Google Cloud Console を開く").clicked() {
                open_url("https://console.cloud.google.com/");
            }
            if ui.button("🌐 OAuth 同意画面を直接開く").clicked() {
                open_url("https://console.cloud.google.com/auth/audience");
            }
            if ui.button("🌐 認証情報ページを直接開く").clicked() {
                open_url("https://console.cloud.google.com/apis/credentials");
            }
        });
        ui.add_space(12.0);

        ui.label(RichText::new("手順 2: クライアント ID / シークレットを貼り付け").strong());
        egui::Grid::new("g_creds").num_columns(2).show(ui, |ui| {
            ui.label("クライアントID:");
            ui.add(
                egui::TextEdit::singleline(&mut self.wizard_gtasks_client_id)
                    .desired_width(360.0)
                    .hint_text("例: 1234-abc.apps.googleusercontent.com"),
            );
            ui.end_row();
            ui.label("クライアントシークレット:");
            ui.add(
                egui::TextEdit::singleline(&mut self.wizard_gtasks_client_secret)
                    .password(true)
                    .desired_width(360.0)
                    .hint_text("例: GOCSPX-xxxxxxxx"),
            );
            ui.end_row();
        });
        ui.add_space(6.0);

        let can_save = !busy
            && !self.wizard_gtasks_client_id.trim().is_empty()
            && !self.wizard_gtasks_client_secret.trim().is_empty();
        if ui
            .add_enabled(
                can_save,
                egui::Button::new("💾 クレデンシャル保存（DPAPI 暗号化）"),
            )
            .clicked()
        {
            let cid = self.wizard_gtasks_client_id.trim().to_string();
            let sec = self.wizard_gtasks_client_secret.trim().to_string();
            self.wizard_gtasks_client_secret.clear();
            self.run_sidecar_command(
                "gtasks",
                vec!["--setup".into(), cid, sec],
                "Google クレデンシャル保存".into(),
            );
        }
        ui.add_space(12.0);

        ui.label(RichText::new("手順 3: ブラウザで認可フロー").strong());
        ui.label("クレデンシャル保存後に下のボタンを押すと、ブラウザが開いて Google の同意画面が出ます。");
        ui.colored_label(
            Color32::from_rgb(220, 180, 80),
            "⚠ 「このアプリは確認されていません」警告は、自分が作ったテストアプリのため正常です。\n   「詳細」→「（アプリ名）に移動」で続行できます（テストユーザー登録が前提）。",
        );
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!busy, egui::Button::new("🌐 ブラウザで認可"))
                .clicked()
            {
                self.run_sidecar_command(
                    "gtasks",
                    vec!["--auth".into()],
                    "Google 認可フロー".into(),
                );
            }
            if ui
                .add_enabled(!busy, egui::Button::new("🔍 現在の状態を確認"))
                .clicked()
            {
                self.run_sidecar_command(
                    "gtasks",
                    vec!["--status".into()],
                    "Google 状態確認".into(),
                );
            }
            if ui
                .add_enabled(!busy, egui::Button::new("🗑 トークン削除"))
                .clicked()
            {
                self.run_sidecar_command(
                    "gtasks",
                    vec!["--clear-auth".into()],
                    "Google トークン削除".into(),
                );
            }
        });

        ui.add_space(12.0);
        self.draw_wizard_status_and_log(ui);
    }

    fn draw_wizard_status_and_log(&mut self, ui: &mut egui::Ui) {
        if let Some(msg) = &self.wizard_status {
            ui.colored_label(Color32::from_rgb(220, 100, 100), msg);
        }
        if let Some(active) = &self.wizard_active {
            ui.separator();
            ui.horizontal(|ui| {
                let elapsed = active
                    .finished_at
                    .map(|f| f.duration_since(active.started_at))
                    .unwrap_or_else(|| active.started_at.elapsed());
                ui.label(RichText::new(format!("実行中: {}", active.name)).strong());
                match active.exit_code {
                    None => {
                        ui.spinner();
                        ui.weak(format!(" ({:.1}秒)", elapsed.as_secs_f32()));
                    }
                    Some(0) => {
                        ui.colored_label(
                            Color32::from_rgb(120, 200, 120),
                            format!("✅ 完了 ({:.1}秒)", elapsed.as_secs_f32()),
                        );
                    }
                    Some(c) => {
                        ui.colored_label(
                            Color32::from_rgb(220, 100, 100),
                            format!("❌ 失敗 (exit {c}, {:.1}秒)", elapsed.as_secs_f32()),
                        );
                    }
                }
            });
            egui::ScrollArea::vertical()
                .max_height(180.0)
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .id_salt("wizard_log_scroll")
                .show(ui, |ui| {
                    if active.log.is_empty() {
                        ui.weak("（出力なし）");
                    } else {
                        for line in &active.log {
                            ui.label(RichText::new(line).monospace().small());
                        }
                    }
                });
        }
    }

    /// サイドカーを起動して標準出力をログに流す。
    /// `is_auto = true` の場合はログウィンドウを自動表示せず、終了時に静かに閉じる。
    fn start_sync(&mut self, sc: &storage::SidecarInfo, is_auto: bool) {
        let (tx, rx) = channel();
        let exe_dir = storage::data_dir_path();
        let mut cmd = Command::new(&sc.path);
        cmd.current_dir(&exe_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Windows: コンソールウィンドウのフラッシュを抑制
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        match cmd.spawn()
        {
            Ok(mut child) => {
                if let Some(out) = child.stdout.take() {
                    let tx2 = tx.clone();
                    thread::spawn(move || {
                        for line in BufReader::new(out).lines().map_while(Result::ok) {
                            if tx2.send(SyncMsg::Line(line)).is_err() {
                                break;
                            }
                        }
                    });
                }
                if let Some(err) = child.stderr.take() {
                    let tx2 = tx.clone();
                    thread::spawn(move || {
                        for line in BufReader::new(err).lines().map_while(Result::ok) {
                            if tx2.send(SyncMsg::Line(format!("[err] {line}"))).is_err() {
                                break;
                            }
                        }
                    });
                }
                let tx_done = tx;
                thread::spawn(move || {
                    let mut child = child;
                    let code = child
                        .wait()
                        .map(|s| s.code().unwrap_or(-1))
                        .unwrap_or(-1);
                    let _ = tx_done.send(SyncMsg::Done(code));
                });
                self.active_sync = Some(ActiveSync {
                    name: sc.display_name.clone(),
                    rx,
                    started_at: Instant::now(),
                    finished_at: None,
                    log: Vec::new(),
                    exit_code: None,
                });
                self.current_sync_is_auto = is_auto;
                self.show_sync_log = !is_auto;
            }
            Err(e) => {
                let (_, dummy_rx) = channel::<SyncMsg>();
                self.active_sync = Some(ActiveSync {
                    name: sc.display_name.clone(),
                    rx: dummy_rx,
                    started_at: Instant::now(),
                    finished_at: Some(Instant::now()),
                    log: vec![format!("起動失敗: {e}")],
                    exit_code: Some(-1),
                });
                self.current_sync_is_auto = is_auto;
                self.show_sync_log = !is_auto;
            }
        }
    }

    /// 同期完了後にディスクから再読込（未保存編集ありなら競合ダイアログを出す）
    fn reload_after_sync(&mut self) {
        let tasks_mt = storage::tasks_mtime();
        if tasks_mt.is_some() && tasks_mt != self.last_tasks_mtime {
            if self.dirty_tasks {
                self.external_conflict = Some(ConflictTarget::Tasks);
            } else {
                // 編集中タスクが消えると id 検索が空になるが念のためクリア
                self.editing_task_date = None;
                self.reload_tasks_from_disk();
            }
        }
        let notes_mt = storage::notes_mtime();
        if notes_mt.is_some() && notes_mt != self.last_notes_mtime {
            if self.dirty_notes {
                if self.external_conflict.is_none() {
                    self.external_conflict = Some(ConflictTarget::Notes);
                }
            } else {
                self.editing_note = None;
                self.viewing_image = None;
                self.reload_notes_from_disk();
            }
        }
    }

    fn add_task(&mut self) {
        let text = self.new_task_text.trim().to_string();
        if text.is_empty() {
            return;
        }
        let order = self.tasks.len() as i64;
        let mut task = Item::new_task(text, order);
        if self.new_task_use_date {
            task.scheduled_date_time = self.new_task_buf.to_dt();
        }
        // PC操作の選択値を保存しつつタスクを追加（PC操作は予定日時 ON 時のみ有効）
        let pc_action = if self.new_task_use_date {
            self.new_task_pc_action
        } else {
            PcAction::None
        };
        self.tasks.push(task);
        let new_idx = self.tasks.len() - 1;
        self.new_task_text.clear();
        self.dirty_tasks = true;
        // PC操作 != None かつ PCReminder 検出済み → 該当コマンドを即時予約。
        // register_pc_action 内で「過去時刻」「pc-reminder 未配置」等も検証し、
        // 失敗時は上部にバナー警告。タスク自体は作成成功のまま残る。
        if pc_action != PcAction::None && self.is_tool_available("pc-reminder-gui") {
            self.register_pc_action(new_idx, pc_action);
        }
        // タスクが追加されたら（追加ボタン・ショートカット問わず）設定の初期値にリセット。
        // タスクごとに PC 操作を再選択させ、前のタスクの選択が次に持ち越されないようにする。
        self.new_task_pc_action = self.settings.pc_action_default;
    }

    fn new_note(&mut self) {
        let order = self.notes.len() as i64;
        let note = Item::new_note(order);
        let id = note.id.clone();
        self.notes.insert(0, note);
        self.selected_note = 0;
        self.renumber_notes();
        self.editing_note = Some(id);
        self.focus_editor = true;
        self.dirty_notes = true;
    }

    fn delete_selected_note(&mut self) {
        if self.selected_note < self.notes.len() {
            let removed = self.notes.remove(self.selected_note);
            // 画像メモなら実体ファイルも削除
            if removed.is_image() {
                storage::delete_image(&removed.task_content);
            }
            let removed_id = removed.id;
            self.renumber_notes();
            if self.selected_note >= self.notes.len() {
                self.selected_note = self.notes.len().saturating_sub(1);
            }
            if self.editing_note.as_deref() == Some(removed_id.as_str()) {
                self.editing_note = None;
            }
            if self.viewing_image.as_deref() == Some(removed_id.as_str()) {
                self.viewing_image = None;
            }
            self.dirty_notes = true;
        }
    }

    /// 画像ファイルをインポートして画像メモとして先頭に追加
    fn add_image_note(&mut self, src: &std::path::Path) -> bool {
        if let Some((rel, original)) = storage::import_image(src) {
            let order = self.notes.len() as i64;
            let item = Item::new_image(rel, original, order);
            self.notes.insert(0, item);
            self.selected_note = 0;
            self.renumber_notes();
            self.dirty_notes = true;
            true
        } else {
            false
        }
    }

    /// ファイルダイアログを開いて画像を選択し、追加する
    fn add_image_via_dialog(&mut self) {
        if let Some(paths) = rfd::FileDialog::new()
            .add_filter("画像", &["png", "jpg", "jpeg", "gif", "bmp", "webp"])
            .set_title("画像メモに追加するファイルを選択")
            .pick_files()
        {
            for p in paths {
                self.add_image_note(&p);
            }
        }
    }

    fn renumber_tasks(&mut self) {
        for (i, t) in self.tasks.iter_mut().enumerate() {
            t.manual_order = i as i64;
        }
    }

    fn renumber_notes(&mut self) {
        for (i, n) in self.notes.iter_mut().enumerate() {
            n.manual_order = i as i64;
        }
    }

    fn persist_tasks(&mut self) {
        self.settings.window_pos = self.win_pos;
        self.settings.window_size = self.win_size;
        self.renumber_tasks();
        storage::save_tasks(&self.tasks);
        storage::save_settings(&self.settings);
        self.last_tasks_mtime = storage::tasks_mtime();
        self.last_saved_tasks = Instant::now();
    }

    fn persist_notes(&mut self) {
        self.settings.window_pos = self.win_pos;
        self.settings.window_size = self.win_size;
        storage::save_notes(&self.notes);
        storage::save_settings(&self.settings);
        self.last_notes_mtime = storage::notes_mtime();
        self.last_saved_notes = Instant::now();
    }

    fn reload_tasks_from_disk(&mut self) {
        let (tasks, _) = storage::load_tasks();
        self.tasks = tasks;
        self.last_tasks_mtime = storage::tasks_mtime();
        self.dirty_tasks = false;
    }

    fn reload_notes_from_disk(&mut self) {
        let (notes, _) = storage::load_notes();
        self.notes = notes;
        if self.selected_note >= self.notes.len() {
            self.selected_note = self.notes.len().saturating_sub(1);
        }
        self.last_notes_mtime = storage::notes_mtime();
        self.dirty_notes = false;
    }

    fn task_display_order(&self) -> Vec<usize> {
        let far_future = NaiveDate::from_ymd_opt(9999, 12, 31)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let mut active: Vec<usize> =
            (0..self.tasks.len()).filter(|&i| !self.tasks[i].is_completed).collect();
        let done: Vec<usize> =
            (0..self.tasks.len()).filter(|&i| self.tasks[i].is_completed).collect();
        match self.settings.sort_mode {
            SortMode::Manual => {}
            SortMode::DateTime => {
                active.sort_by(|&a, &b| {
                    let ka = self.tasks[a].scheduled_date_time.unwrap_or(far_future);
                    let kb = self.tasks[b].scheduled_date_time.unwrap_or(far_future);
                    ka.cmp(&kb)
                        .then(self.tasks[b].priority.rank().cmp(&self.tasks[a].priority.rank()))
                });
            }
            SortMode::Priority => {
                active.sort_by(|&a, &b| {
                    let pa = self.tasks[a].priority.rank();
                    let pb = self.tasks[b].priority.rank();
                    let ka = self.tasks[a].scheduled_date_time.unwrap_or(far_future);
                    let kb = self.tasks[b].scheduled_date_time.unwrap_or(far_future);
                    pb.cmp(&pa).then(ka.cmp(&kb))
                });
            }
        }
        active.extend(done);
        active
    }
}

fn color_of(arr: [u8; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(arr[0], arr[1], arr[2], arr[3])
}

fn dim(c: Color32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), c.a() / 2)
}

fn format_schedule(
    dt: Option<NaiveDateTime>,
    far: FarFormat,
    near: NearFormat,
    now: NaiveDateTime,
) -> (String, bool) {
    let Some(t) = dt else {
        return ("—".to_string(), false);
    };
    let secs = t.signed_duration_since(now).num_seconds();
    let overdue = secs < 0;
    let asecs = secs.abs();
    let hours = asecs / 3600;
    let days = asecs / 86400;
    let text = if hours >= 24 {
        match far {
            FarFormat::Date => t.format("%m/%d").to_string(),
            FarFormat::RelativeDays => {
                if overdue {
                    format!("{}日超過", days)
                } else {
                    format!("残り{}日", days)
                }
            }
        }
    } else {
        match near {
            NearFormat::Time => t.format("%H:%M").to_string(),
            NearFormat::RelativeHours => {
                if overdue {
                    format!("{}時間超過", hours)
                } else {
                    format!("残り{}時間", hours)
                }
            }
        }
    };
    (text, overdue)
}

enum RowAction {
    SetPriority(Priority),
    EditDate,
    Delete,
    SetReminder,
    ClearReminder,
}

fn row_menu(
    ui: &mut egui::Ui,
    has_reminder: bool,
    pcr_available: bool,
) -> Option<RowAction> {
    let mut action = None;
    ui.label("重要度を設定");
    for p in [Priority::High, Priority::Medium, Priority::Low, Priority::None] {
        if ui.button(p.label()).clicked() {
            action = Some(RowAction::SetPriority(p));
            ui.close_menu();
        }
    }
    ui.separator();
    if ui.button("📅 予定日時を編集").clicked() {
        action = Some(RowAction::EditDate);
        ui.close_menu();
    }
    // リマインダー項目: PCReminder 検出時のみ「登録」を出す。
    // 既に登録済 (reminder_key が残っている) なら、PCReminder が未検出でも
    // 「解除」だけは表示してローカル状態をクリーンアップできるようにする。
    if has_reminder {
        if ui.button("🔕 リマインダーを解除").clicked() {
            action = Some(RowAction::ClearReminder);
            ui.close_menu();
        }
    } else if pcr_available && ui.button("🔔 リマインダーを登録").clicked() {
        action = Some(RowAction::SetReminder);
        ui.close_menu();
    }
    if ui.button("🗑 削除").clicked() {
        action = Some(RowAction::Delete);
        ui.close_menu();
    }
    action
}

fn fmt_updated(s: &str) -> String {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Local).format("%Y/%m/%d %H:%M").to_string())
        .unwrap_or_else(|_| "—".to_string())
}

fn theme_pref(m: ThemeMode) -> egui::ThemePreference {
    match m {
        ThemeMode::System => egui::ThemePreference::System,
        ThemeMode::Dark => egui::ThemePreference::Dark,
        ThemeMode::Light => egui::ThemePreference::Light,
    }
}

fn apply_font(ctx: &egui::Context, family_name: &str) {
    let mut fonts = egui::FontDefinitions::default();
    if let Some(path) = font_path_for(family_name) {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert("jp".to_owned(), egui::FontData::from_owned(bytes));
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "jp".to_owned());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .insert(0, "jp".to_owned());
        }
    }
    ctx.set_fonts(fonts);
}

fn clamp_pos(pos: [f32; 2], size: [f32; 2], mon: [f32; 2]) -> [f32; 2] {
    let maxx = (mon[0] - size[0]).max(0.0);
    let maxy = (mon[1] - size[1]).max(0.0);
    [pos[0].clamp(0.0, maxx), pos[1].clamp(0.0, maxy)]
}

/// テキスト中の URL を検出してテキスト/URL のセグメント列に分解する。
/// http:// または https:// を URL とみなし、空白文字直前まで取り込む。
/// 末尾の句読点・括弧（"。、，．,.;:!?)]}」』>"）は URL から外す。
pub enum TextSegment {
    Text(String),
    Url(String),
}

/// 与えられた幅 `max_width` (px) に収まるよう文字単位で切り詰めて
/// 末尾に "…" を付けた文字列を返す（全文が収まればそのまま返す）。
///
/// 用途: egui の `Label::truncate()` は省略時に自動で全文ツールチップを出し、
/// 本文プレビューと二重表示になるため、それを避けるための自前 truncate。
fn truncate_to_fit(ui: &egui::Ui, text: &str, max_width: f32) -> String {
    if text.is_empty() {
        return String::new();
    }
    let font_id = ui
        .style()
        .text_styles
        .get(&egui::TextStyle::Body)
        .cloned()
        .unwrap_or_else(|| egui::FontId::default());
    ui.fonts(|fonts| {
        let full_w: f32 = text
            .chars()
            .map(|c| fonts.glyph_width(&font_id, c))
            .sum();
        if full_w <= max_width {
            return text.to_string();
        }
        let ellipsis_w = fonts.glyph_width(&font_id, '…');
        let budget = (max_width - ellipsis_w).max(0.0);
        let mut width = 0.0_f32;
        let mut out = String::with_capacity(text.len());
        for ch in text.chars() {
            let gw = fonts.glyph_width(&font_id, ch);
            if width + gw > budget {
                break;
            }
            width += gw;
            out.push(ch);
        }
        out.push('…');
        out
    })
}

pub fn split_url_segments(s: &str) -> Vec<TextSegment> {
    let mut out: Vec<TextSegment> = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0usize;
    let mut buf = String::new();
    while i < bytes.len() {
        // i は char 境界（ASCII プレフィックス検査のため境界判定は不要）
        let rest = &s[i..];
        let scheme_len = if rest.starts_with("https://") {
            "https://".len()
        } else if rest.starts_with("http://") {
            "http://".len()
        } else {
            0
        };
        if scheme_len > 0 {
            // URL 開始: 空白までを切り出す
            let end_rel = rest
                .find(|c: char| c.is_whitespace())
                .unwrap_or(rest.len());
            let mut url = &rest[..end_rel];
            // 末尾のノイズ記号を削る（マッチした分だけ後ろから戻す）
            let trim_chars: &[char] = &[
                '。', '、', ',', '.', ';', ':', '!', '?', ')', ']', '}',
                '」', '』', '>', '"', '\'',
            ];
            while let Some(last) = url.chars().last() {
                if trim_chars.contains(&last) {
                    url = &url[..url.len() - last.len_utf8()];
                } else {
                    break;
                }
            }
            // スキームより長い = ホスト部があるならリンク化、
            // ちょうどスキームだけ（例: "https://"）ならテキスト扱い。
            if url.len() <= scheme_len {
                buf.push_str(&rest[..end_rel]);
                i += end_rel;
                continue;
            }
            // 直前まで溜まったテキストを flush
            if !buf.is_empty() {
                out.push(TextSegment::Text(std::mem::take(&mut buf)));
            }
            out.push(TextSegment::Url(url.to_string()));
            // 削った末尾記号 + URL 後の残りはテキストに送る
            let consumed_url_full = &rest[..end_rel];
            let tail = &consumed_url_full[url.len()..];
            if !tail.is_empty() {
                buf.push_str(tail);
            }
            i += end_rel;
        } else {
            // 1 char 進める
            let ch = rest.chars().next().unwrap();
            buf.push(ch);
            i += ch.len_utf8();
        }
    }
    if !buf.is_empty() {
        out.push(TextSegment::Text(buf));
    }
    out
}

#[cfg(windows)]
fn open_url(url: &str) {
    // `cmd /C start "" URL` は URL 内の `&` を cmd がコマンド区切りとして
    // 解釈してしまうため避ける（クエリ文字列が途切れる）。rundll32 経由で
    // Windows のシェル関連付けに渡す方式が安全。
    let _ = std::process::Command::new("rundll32.exe")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn();
}

#[cfg(not(windows))]
fn open_url(url: &str) {
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

/// 画像ファイルパスから file:// URI を作る（egui_extras のローダー用）
fn image_uri(item: &Item) -> String {
    let p: PathBuf = storage::image_abs_path(&item.task_content);
    // Windows パスを URI 化（バックスラッシュ→スラッシュ）
    let s = p.to_string_lossy().replace('\\', "/");
    format!("file://{s}")
}

/// 中央クロップ用の UV 矩形を計算（元画像と表示サイズのアスペクト比から）
fn crop_uv(img: egui::Vec2, dst: egui::Vec2) -> egui::Rect {
    let src_ratio = img.x / img.y;
    let dst_ratio = dst.x / dst.y;
    if src_ratio > dst_ratio {
        // 元画像が横長すぎ → 左右をカット
        let visible = dst_ratio / src_ratio;
        let pad = (1.0 - visible) / 2.0;
        egui::Rect::from_min_max(egui::pos2(pad, 0.0), egui::pos2(1.0 - pad, 1.0))
    } else {
        // 元画像が縦長 → 上下をカット
        let visible = src_ratio / dst_ratio;
        let pad = (1.0 - visible) / 2.0;
        egui::Rect::from_min_max(egui::pos2(0.0, pad), egui::pos2(1.0, 1.0 - pad))
    }
}

/// サムネイルを指定矩形に描画する。クロップは UV で、ストレッチは fit_to_exact_size で実現。
fn draw_thumbnail(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    item: &Item,
    rect: egui::Rect,
    shape: ThumbShape,
) {
    let uri = image_uri(item);
    let dst = rect.size();
    // 背景塗り（読み込み失敗・透明画像のフォールバック）
    ui.painter().rect_filled(rect, 1.0, ui.visuals().extreme_bg_color);

    match shape {
        ThumbShape::Stretch => {
            let img = egui::Image::new(uri).fit_to_exact_size(dst);
            img.paint_at(ui, rect);
        }
        ThumbShape::Crop => {
            // 元画像のサイズを取得して中央クロップ UV を計算
            let texture_result = ctx.try_load_texture(
                &uri,
                egui::TextureOptions::LINEAR,
                egui::SizeHint::Scale(1.0.into()),
            );
            match texture_result {
                Ok(egui::load::TexturePoll::Ready { texture }) => {
                    let uv = crop_uv(texture.size, dst);
                    let img = egui::Image::new(uri).uv(uv).fit_to_exact_size(dst);
                    img.paint_at(ui, rect);
                }
                _ => {
                    // 読み込み中 → ストレッチで仮表示
                    let img = egui::Image::new(uri).fit_to_exact_size(dst);
                    img.paint_at(ui, rect);
                }
            }
        }
    }
    // 枠線
    ui.painter()
        .rect_stroke(rect, 1.0, egui::Stroke::new(1.0, ui.visuals().weak_text_color()));
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // フレーム冒頭でプレビュー要求をリセット（ホバー検出側が再セットする）
        self.pending_preview = None;

        // ===== ウィンドウ状態の観測 =====
        let focused = ctx.input(|i| {
            let vp = i.viewport();
            if let Some(o) = vp.outer_rect {
                self.win_pos = Some([o.min.x, o.min.y]);
            }
            if let Some(inner) = vp.inner_rect {
                self.win_size = Some([inner.width(), inner.height()]);
            }
            if let Some(mon) = vp.monitor_size {
                self.monitor_size = Some([mon.x, mon.y]);
            }
            vp.focused.unwrap_or(true)
        });

        // ===== 起動時: ウィンドウ位置クランプ =====
        if !self.geometry_checked {
            if let (Some(mon), Some(pos), Some(sz)) =
                (self.monitor_size, self.win_pos, self.win_size)
            {
                let c = clamp_pos(pos, sz, mon);
                if (c[0] - pos[0]).abs() > 1.0 || (c[1] - pos[1]).abs() > 1.0 {
                    ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
                        c[0], c[1],
                    )));
                }
                self.geometry_checked = true;
            }
        }

        // ===== フォーカス復帰時: 外部変更検知 =====
        if focused && !self.was_focused && self.external_conflict.is_none() {
            let tasks_mt = storage::tasks_mtime();
            if tasks_mt.is_some() && tasks_mt != self.last_tasks_mtime {
                if self.dirty_tasks {
                    self.external_conflict = Some(ConflictTarget::Tasks);
                } else {
                    self.reload_tasks_from_disk();
                }
            }
            if self.external_conflict.is_none() {
                let notes_mt = storage::notes_mtime();
                if notes_mt.is_some() && notes_mt != self.last_notes_mtime {
                    if self.dirty_notes {
                        self.external_conflict = Some(ConflictTarget::Notes);
                    } else {
                        self.reload_notes_from_disk();
                    }
                }
            }
        }
        self.was_focused = focused;

        // ===== テーマ変更の反映 =====
        if self.settings.theme != self.current_theme {
            ctx.set_theme(theme_pref(self.settings.theme));
            self.current_theme = self.settings.theme;
        }

        // ===== 常に最前面の反映 =====
        if self.settings.always_on_top != self.current_on_top {
            let level = if self.settings.always_on_top {
                egui::WindowLevel::AlwaysOnTop
            } else {
                egui::WindowLevel::Normal
            };
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
            self.current_on_top = self.settings.always_on_top;
        }

        // ===== フォントサイズを毎フレーム反映 =====
        let size = self.settings.font_size;
        ctx.style_mut(|s| {
            use egui::FontFamily::Proportional;
            use egui::TextStyle::*;
            s.text_styles = [
                (Heading, FontId::new(size * 1.3, Proportional)),
                (Body, FontId::new(size, Proportional)),
                (Button, FontId::new(size, Proportional)),
                (Monospace, FontId::new(size, egui::FontFamily::Monospace)),
                (Small, FontId::new(size * 0.85, Proportional)),
            ]
            .into();
            // スクロールバーを常時表示に。
            // floating=false にして 8px 幅の固定バーにする。
            // ハンドル（位置を示す白いつまみ）は foreground 色＋常時不透明、
            // 背景レール（track）は薄い半透明で常時うっすら見える状態。
            let sc = &mut s.spacing.scroll;
            sc.floating = false;
            sc.bar_width = 8.0;
            sc.foreground_color = true;
            sc.dormant_background_opacity = 0.3;
            sc.dormant_handle_opacity = 1.0;
            sc.interact_background_opacity = 0.4;
            sc.interact_handle_opacity = 1.0;
            sc.active_background_opacity = 0.5;
            sc.active_handle_opacity = 1.0;
        });

        // ===== タブバー =====
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                if ui.selectable_label(self.mode == Mode::Task, "📋 タスク").clicked() {
                    self.mode = Mode::Task;
                }
                if ui.selectable_label(self.mode == Mode::Note, "📝 メモ").clicked() {
                    self.mode = Mode::Note;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("⚙ 設定").clicked() {
                        self.show_settings = !self.show_settings;
                    }
                    // 🔄 同期ボタン
                    let has = !self.sidecars.is_empty();
                    let running = self
                        .active_sync
                        .as_ref()
                        .map(|a| a.exit_code.is_none())
                        .unwrap_or(false);
                    let btn = ui.add_enabled(
                        has && !running,
                        egui::Button::new(if running { "🔄 同期中…" } else { "🔄 同期" }),
                    );
                    let popup_id = egui::Id::new("sync_menu");
                    if has {
                        if btn.clicked() {
                            ui.memory_mut(|m| m.open_popup(popup_id));
                        }
                        let mut run_idx: Option<usize> = None;
                        let mut do_refresh = false;
                        egui::popup_below_widget(
                            ui,
                            popup_id,
                            &btn,
                            egui::PopupCloseBehavior::CloseOnClickOutside,
                            |ui| {
                                ui.set_min_width(160.0);
                                ui.label(RichText::new("同期アダプター").weak().small());
                                for (i, sc) in self.sidecars.iter().enumerate() {
                                    if ui.button(format!("▶ {}", sc.display_name)).clicked() {
                                        run_idx = Some(i);
                                        ui.memory_mut(|m| m.close_popup());
                                    }
                                }
                                ui.separator();
                                if ui.button("🔄 リスト更新").clicked() {
                                    do_refresh = true;
                                    ui.memory_mut(|m| m.close_popup());
                                }
                                if self.active_sync.is_some() {
                                    if ui.button("📜 直近ログを表示").clicked() {
                                        self.show_sync_log = true;
                                        ui.memory_mut(|m| m.close_popup());
                                    }
                                }
                            },
                        );
                        if let Some(i) = run_idx {
                            if let Some(sc) = self.sidecars.get(i).cloned() {
                                self.start_sync(&sc, false);
                            }
                        }
                        if do_refresh {
                            self.sidecars = storage::discover_sidecars();
                        }
                    } else {
                        btn.on_hover_text(
                            "sync/uninote-sync-<service>.exe を配置すると有効になります",
                        );
                    }
                });
            });
            ui.add_space(2.0);
        });

        // ===== モード別ツールバー =====
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.add_space(3.0);
            match self.mode {
                Mode::Task => {
                    // ── 1段目: 関連ツール起動ボタン + 並び ──
                    ui.horizontal_wrapped(|ui| {
                        // 関連ツール（プラグイン）起動ボタン
                        if !self.tools.is_empty() {
                            let mut launch_idx: Option<usize> = None;
                            for (i, t) in self.tools.iter().enumerate() {
                                if ui
                                    .button(t.icon)
                                    .on_hover_text(&t.display_name)
                                    .clicked()
                                {
                                    launch_idx = Some(i);
                                }
                            }
                            if let Some(i) = launch_idx {
                                if let Some(t) = self.tools.get(i).cloned() {
                                    self.launch_tool(&t);
                                }
                            }
                            ui.separator();
                        }
                        ui.label("並び:");
                        egui::ComboBox::from_id_salt("sort")
                            .selected_text(self.settings.sort_mode.label())
                            .show_ui(ui, |ui| {
                                for m in
                                    [SortMode::Manual, SortMode::DateTime, SortMode::Priority]
                                {
                                    if ui
                                        .selectable_value(
                                            &mut self.settings.sort_mode,
                                            m,
                                            m.label(),
                                        )
                                        .changed()
                                    {
                                        self.dirty_tasks = true;
                                    }
                                }
                            });
                    });
                    // ── 2段目: 予定日時。非折り返しの ui.horizontal を行単位で並べる。
                    //    日付・時刻は atomic（途中で折り返さない=要件）。
                    //    幅を安全側に過大評価して事前に行割りするので、時刻は絶対に
                    //    "時:分" の途中で折り返されない。 ──
                    let use_date = self.new_task_use_date;
                    // 予定日時 OFF なら PC操作も自動で「通知なし」に戻す
                    if !use_date && self.new_task_pc_action != PcAction::None {
                        self.new_task_pc_action = PcAction::None;
                    }
                    #[derive(Clone, Copy, PartialEq)]
                    enum Atom {
                        Check,
                        Date,
                        Time,
                        Now,
                        Pc,
                    }
                    // 各要素の概算幅。DragValue を add（自然サイズ）で描画するため
                    // 余裕を持って大きめに設定（時刻消失防止）。
                    // PC操作ボタンは PCReminder 検出時のみ含める。
                    let mut atoms: Vec<(Atom, f32)> = vec![
                        (Atom::Check, 100.0),
                        (Atom::Date, 130.0),
                        (Atom::Time, 76.0),
                        (Atom::Now, 70.0),
                    ];
                    if self.is_tool_available("pc-reminder-gui") {
                        atoms.push((Atom::Pc, 84.0));
                    }
                    let avail = ui.available_width();
                    let mut rows: Vec<Vec<Atom>> = Vec::new();
                    let mut cur: Vec<Atom> = Vec::new();
                    let mut used = 0.0_f32;
                    for (a, w) in atoms {
                        if !cur.is_empty() && used + w > avail {
                            rows.push(std::mem::take(&mut cur));
                            used = 0.0;
                        }
                        cur.push(a);
                        used += w;
                    }
                    if !cur.is_empty() {
                        rows.push(cur);
                    }
                    for row in rows {
                        let has_check = row.contains(&Atom::Check);
                        ui.horizontal(|ui| {
                            if !has_check && !use_date {
                                ui.disable();
                            }
                            for a in row {
                                match a {
                                    Atom::Check => {
                                        ui.checkbox(
                                            &mut self.new_task_use_date,
                                            "予定日時",
                                        );
                                        if !use_date {
                                            ui.disable();
                                        }
                                    }
                                    Atom::Date => {
                                        date_part(ui, &mut self.new_task_buf);
                                    }
                                    Atom::Time => {
                                        time_part(ui, &mut self.new_task_buf);
                                    }
                                    Atom::Now => {
                                        if ui.small_button("今すぐ").clicked() {
                                            self.new_task_buf = DateBuf::now();
                                        }
                                    }
                                    Atom::Pc => {
                                        // クリックで「通知なし→通知あり→起動→スリープ→…」を循環。
                                        // タスクが追加された時点の選択値で予約される
                                        // （追加ボタンクリック・ショートカット入力どちらでも同じ）。
                                        let label = self.new_task_pc_action.label();
                                        if ui
                                            .button(label)
                                            .on_hover_text(
                                                "クリックで PC 操作を切替\n通知なし / 通知あり / 起動(+通知) / スリープ\n→ tools/pc-reminder.exe が必要",
                                            )
                                            .clicked()
                                        {
                                            self.new_task_pc_action =
                                                self.new_task_pc_action.next();
                                        }
                                    }
                                }
                            }
                        });
                    }
                    self.new_task_buf.clamp();
                    // ── 3段目: タスク入力欄 + 追加 ──
                    ui.horizontal_wrapped(|ui| {
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut self.new_task_text)
                                .hint_text("新しいタスクを入力")
                                .desired_width(200.0),
                        );
                        // 登録キー設定に応じて Enter 系での登録を判定（既定: なし）
                        let key_add = resp.lost_focus()
                            && ui.input(|i| match self.settings.add_task_key {
                                AddTaskKey::None => false,
                                AddTaskKey::Enter => {
                                    i.key_pressed(egui::Key::Enter)
                                        && !i.modifiers.shift
                                        && !i.modifiers.ctrl
                                }
                                AddTaskKey::ShiftEnter => {
                                    i.key_pressed(egui::Key::Enter) && i.modifiers.shift
                                }
                                AddTaskKey::CtrlEnter => {
                                    i.key_pressed(egui::Key::Enter) && i.modifiers.ctrl
                                }
                            });
                        if ui.button("追加").clicked() || key_add {
                            self.add_task();
                            resp.request_focus();
                        }
                    });
                }
                Mode::Note => {
                    ui.horizontal_wrapped(|ui| {
                        let has = !self.notes.is_empty();
                        if ui.button("📄 新規作成").clicked() {
                            self.new_note();
                        }
                        if ui.button("🖼 画像を追加").clicked() {
                            self.add_image_via_dialog();
                        }
                        if ui.add_enabled(has, egui::Button::new("✏ 開く")).clicked() {
                            if let Some(n) = self.notes.get(self.selected_note).cloned() {
                                if n.is_image() {
                                    self.viewing_image = Some(n.id);
                                } else {
                                    self.editing_note = Some(n.id);
                                    self.focus_editor = true;
                                }
                            }
                        }
                        if ui.add_enabled(has, egui::Button::new("🗑 削除")).clicked() {
                            self.delete_selected_note();
                        }
                    });
                }
            }
            ui.add_space(3.0);
        });

        // ===== ステータスメッセージ =====
        if let Some(msg) = self.status_msg.clone() {
            egui::TopBottomPanel::top("status").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(Color32::from_rgb(230, 170, 60), format!("⚠ {msg}"));
                    if ui.button("✕").clicked() {
                        self.status_msg = None;
                    }
                });
            });
        }

        // ===== リマインダー登録/解除の結果通知 =====
        if let Some(msg) = self.reminder_status.clone() {
            let color = if self.reminder_status_is_error {
                Color32::from_rgb(220, 100, 100)
            } else {
                Color32::from_rgb(120, 200, 120)
            };
            egui::TopBottomPanel::top("reminder_status").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(color, msg);
                    if ui.button("✕").clicked() {
                        self.reminder_status = None;
                    }
                });
            });
        }

        // ===== メインコンテンツ =====
        let now = Local::now().naive_local();
        let far = self.settings.far_format;
        let near = self.settings.near_format;
        let colors = self.settings.colors;
        let manual_mode = self.settings.sort_mode == SortMode::Manual;
        let order = self.task_display_order();

        // タスクリスト用のホバー追跡（手動 DnD 用）
        let mut hovered_row: Option<usize> = None;
        // 時刻ラベルクリックで日時編集ダイアログを開く用
        let mut open_date_idx: Option<usize> = None;
        // メモリスト用
        let mut open_note_id: Option<String> = None;
        // タスク内容ビューワを開く対象（既存がある場合は閉じてから差し替え）
        let mut open_view_task_id: Option<String> = None;

        egui::CentralPanel::default().show(ctx, |ui| {
            match self.mode {
                // ─── タスク一覧 ───
                Mode::Task => {
                    if self.tasks.is_empty() {
                        ui.add_space(20.0);
                        ui.vertical_centered(|ui| {
                            ui.weak("タスクはまだありません。上の入力欄から追加してください。");
                        });
                        return;
                    }
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let (_, released) =
                                ui.dnd_drop_zone::<usize, ()>(egui::Frame::none(), |ui| {
                                    for &i in &order {
                                        let task_id = self.tasks[i].id.clone();
                                        let task = &mut self.tasks[i];
                                        let bg = match task.priority {
                                            Priority::High => colors.high,
                                            Priority::Medium => colors.medium,
                                            Priority::Low => colors.low,
                                            Priority::None => colors.none,
                                        };
                                        let completed = task.is_completed;
                                        let fill = if completed {
                                            dim(color_of(bg))
                                        } else {
                                            color_of(bg)
                                        };
                                        let dirty = &mut self.dirty_tasks;
                                        let open_date_ref = &mut open_date_idx;
                                        let open_view_ref = &mut open_view_task_id;
                                        let preview_ref = &mut self.pending_preview;
                                        let fr = egui::Frame::none()
                                            .fill(fill)
                                            .inner_margin(egui::Margin::symmetric(6.0, 4.0))
                                            .show(ui, |ui| {
                                                ui.horizontal(|ui| {
                                                    if ui
                                                        .checkbox(&mut task.is_completed, "")
                                                        .changed()
                                                    {
                                                        task.touch();
                                                        *dirty = true;
                                                    }
                                                    let (sched, overdue) = format_schedule(
                                                        task.scheduled_date_time,
                                                        far,
                                                        near,
                                                        now,
                                                    );
                                                    let mut rt = RichText::new(
                                                        format!("{sched:>8}"),
                                                    )
                                                    .monospace();
                                                    if task.is_completed {
                                                        rt = rt.weak();
                                                    } else if overdue {
                                                        rt = rt.color(Color32::from_rgb(
                                                            240, 90, 90,
                                                        ));
                                                    }
                                                    if ui
                                                        .add(
                                                            egui::Label::new(rt)
                                                                .sense(egui::Sense::click()),
                                                        )
                                                        .on_hover_text("クリックで予定日時を編集")
                                                        .clicked()
                                                    {
                                                        *open_date_ref = Some(i);
                                                    }
                                                    if task.reminder_key.is_some() {
                                                        ui.label(
                                                            RichText::new("🔔").small(),
                                                        )
                                                        .on_hover_text(
                                                            "リマインダー登録済（右クリックで解除）",
                                                        );
                                                    }
                                                    ui.with_layout(
                                                        egui::Layout::right_to_left(
                                                            egui::Align::Center,
                                                        ),
                                                        |ui| {
                                                            if manual_mode {
                                                                ui.dnd_drag_source(
                                                                    egui::Id::new(("drag", &task_id)),
                                                                    i,
                                                                    |ui| {
                                                                        ui.label(
                                                                            RichText::new("⠿")
                                                                                .weak(),
                                                                        )
                                                                        .on_hover_text(
                                                                            "ドラッグで並び替え",
                                                                        );
                                                                    },
                                                                );
                                                            }
                                                            // 完了・未完了とも Label に統一。
                                                            // クリック = ミニウィンドウ起動（編集はミニ内で行う）。
                                                            // egui の .truncate() は省略時に
                                                            // 自動で全文ツールチップを出してしまう。
                                                            // tooltip_delay 上書きは
                                                            // tooltip_grace_time のせいで他ツール
                                                            // チップ直後だと効かないため、
                                                            // 自前で幅計算→文字数truncate→
                                                            // wrap_mode(Extend) で表示する
                                                            // （elision 自体を起こさない）。
                                                            // さらに親が right_to_left なので、
                                                            // 残りスペース全体を left_to_right で
                                                            // 切り取り、その中で左揃え描画する。
                                                            // ドラッグハンドル ⠿ との視覚的な
                                                            // かぶりを避けるため、ラベル領域から
                                                            // 12px ぶん余白を引いておく。
                                                            const HANDLE_GAP: f32 = 12.0;
                                                            let avail_w =
                                                                (ui.available_width() - HANDLE_GAP)
                                                                    .max(40.0);
                                                            let row_h =
                                                                ui.spacing().interact_size.y;
                                                            let display = truncate_to_fit(
                                                                ui,
                                                                &task.task_content,
                                                                avail_w,
                                                            );
                                                            let rt = if task.is_completed {
                                                                RichText::new(&display)
                                                                    .strikethrough()
                                                                    .weak()
                                                            } else {
                                                                RichText::new(&display)
                                                            };
                                                            ui.allocate_ui_with_layout(
                                                                egui::vec2(avail_w, row_h),
                                                                egui::Layout::left_to_right(
                                                                    egui::Align::Center,
                                                                ),
                                                                |ui| {
                                                                    let lbl_resp = ui.add(
                                                                        egui::Label::new(rt)
                                                                            .wrap_mode(
                                                                                egui::TextWrapMode::Extend,
                                                                            )
                                                                            .selectable(false)
                                                                            .sense(
                                                                                egui::Sense::click(),
                                                                            ),
                                                                    );
                                                                    if lbl_resp.hovered() {
                                                                        *preview_ref = Some(
                                                                            task.task_content
                                                                                .clone(),
                                                                        );
                                                                    }
                                                                    if lbl_resp.clicked() {
                                                                        *open_view_ref = Some(
                                                                            task_id.clone(),
                                                                        );
                                                                    }
                                                                    // 残り空白部分もクリック/
                                                                    // ホバーターゲットに含める。
                                                                    let remaining_w =
                                                                        ui.available_width();
                                                                    if remaining_w > 1.0 {
                                                                        let (rect, gap_resp) =
                                                                            ui.allocate_exact_size(
                                                                                egui::vec2(
                                                                                    remaining_w,
                                                                                    row_h,
                                                                                ),
                                                                                egui::Sense::click(),
                                                                            );
                                                                        let _ = rect;
                                                                        if gap_resp.hovered() {
                                                                            *preview_ref = Some(
                                                                                task.task_content
                                                                                    .clone(),
                                                                            );
                                                                        }
                                                                        if gap_resp.clicked() {
                                                                            *open_view_ref =
                                                                                Some(
                                                                                    task_id
                                                                                        .clone(),
                                                                                );
                                                                        }
                                                                    }
                                                                },
                                                            );
                                                        },
                                                    );
                                                });
                                            });
                                        let row_resp = fr.response;
                                        if row_resp.contains_pointer() {
                                            hovered_row = Some(i);
                                        }
                                        // 右クリック検出 → 別ウィンドウメニューを開く準備
                                        // （位置は行の左下のスクリーン絶対座標）
                                        if row_resp.contains_pointer()
                                            && ui.input(|inp| inp.pointer.secondary_clicked())
                                        {
                                            let inner_min = ui
                                                .input(|i| i.viewport().inner_rect)
                                                .map(|r| r.min)
                                                .unwrap_or(egui::pos2(0.0, 0.0));
                                            let sx = inner_min.x + row_resp.rect.left();
                                            let sy = inner_min.y + row_resp.rect.bottom();
                                            self.context_menu_task_id =
                                                Some(task_id.clone());
                                            self.context_menu_screen_pos =
                                                Some([sx, sy]);
                                        }
                                    }
                                });

                            if manual_mode {
                                if let Some(payload) = released {
                                    let from = *payload;
                                    let to = hovered_row
                                        .unwrap_or(self.tasks.len().saturating_sub(1));
                                    if from < self.tasks.len() && from != to {
                                        let item = self.tasks.remove(from);
                                        let dest = to.min(self.tasks.len());
                                        self.tasks.insert(dest, item);
                                        self.renumber_tasks();
                                        self.dirty_tasks = true;
                                    }
                                }
                            }
                        });
                }
                // ─── メモ一覧 ───
                Mode::Note => {
                    if self.notes.is_empty() {
                        ui.add_space(24.0);
                        ui.vertical_centered(|ui| {
                            ui.weak("メモがありません。「📄 新規作成」または「🖼 画像を追加」で追加してください。");
                        });
                        return;
                    }
                    let img_disp = self.settings.image_display;
                    let thumb_shape = self.settings.thumb_shape;
                    let thumb_h = self.settings.thumb_height.clamp(24.0, 120.0);
                    let thumb_w = thumb_h * 2.5;
                    let font_size = self.settings.font_size;
                    let mut hovered_note: Option<usize> = None;

                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let (_, released) =
                                ui.dnd_drop_zone::<usize, ()>(egui::Frame::none(), |ui| {
                                    for i in 0..self.notes.len() {
                                        let selected = i == self.selected_note;
                                        let note = self.notes[i].clone();
                                        let updated = fmt_updated(&note.updated_at);
                                        let drag_id =
                                            egui::Id::new(("note_drag", &note.id));

                                        let inner_resp =
                                            ui.dnd_drag_source(drag_id, i, |ui| {
                                                ui.horizontal(|ui| {
                                                    // 左端の並び替えハンドル
                                                    ui.label(
                                                        RichText::new("⠿").weak(),
                                                    )
                                                    .on_hover_text("ドラッグで並び替え");

                                                    if note.is_image()
                                                        && img_disp == ImageDisplay::Thumbnail
                                                    {
                                                        // ── サムネイル表示 ──
                                                        let row_h = thumb_h + 6.0;
                                                        let (rect, r) = ui
                                                            .allocate_exact_size(
                                                                egui::vec2(
                                                                    ui.available_width(),
                                                                    row_h,
                                                                ),
                                                                egui::Sense::click(),
                                                            );
                                                        if selected {
                                                            ui.painter().rect_filled(
                                                                rect,
                                                                2.0,
                                                                ui.visuals().selection.bg_fill,
                                                            );
                                                        } else if r.hovered() {
                                                            ui.painter().rect_filled(
                                                                rect,
                                                                2.0,
                                                                ui.visuals()
                                                                    .widgets
                                                                    .hovered
                                                                    .bg_fill,
                                                            );
                                                        }
                                                        let thumb_rect =
                                                            egui::Rect::from_min_size(
                                                                rect.min
                                                                    + egui::vec2(4.0, 3.0),
                                                                egui::vec2(thumb_w, thumb_h),
                                                            );
                                                        draw_thumbnail(
                                                            ui,
                                                            ctx,
                                                            &note,
                                                            thumb_rect,
                                                            thumb_shape,
                                                        );
                                                        let name = note
                                                            .image_name
                                                            .clone()
                                                            .unwrap_or_else(|| "(画像)".into());
                                                        let text_rect =
                                                            egui::Rect::from_min_max(
                                                                egui::pos2(
                                                                    thumb_rect.max.x + 8.0,
                                                                    rect.min.y,
                                                                ),
                                                                egui::pos2(
                                                                    rect.max.x - 4.0,
                                                                    rect.max.y,
                                                                ),
                                                            );
                                                        ui.painter().text(
                                                            text_rect.left_center(),
                                                            egui::Align2::LEFT_CENTER,
                                                            name,
                                                            FontId::proportional(font_size),
                                                            ui.visuals().text_color(),
                                                        );
                                                        r.on_hover_text(format!(
                                                            "更新日時: {updated}"
                                                        ))
                                                    } else {
                                                        // ── ファイル名 / テキストメモ ──
                                                        let title = if note.is_image() {
                                                            format!(
                                                                "🖼 {}",
                                                                note.image_name
                                                                    .clone()
                                                                    .unwrap_or_else(|| "(画像)"
                                                                        .into())
                                                            )
                                                        } else {
                                                            note.title()
                                                        };
                                                        ui.add_sized(
                                                            [ui.available_width(), 24.0],
                                                            egui::SelectableLabel::new(
                                                                selected,
                                                                RichText::new(title),
                                                            ),
                                                        )
                                                        .on_hover_text(format!(
                                                            "更新日時: {updated}"
                                                        ))
                                                    }
                                                })
                                                .inner
                                            });

                                        let outer = inner_resp.response;
                                        if outer.contains_pointer() {
                                            hovered_note = Some(i);
                                        }
                                        let resp = inner_resp.inner;
                                        if resp.clicked() {
                                            self.selected_note = i;
                                        }
                                        if resp.double_clicked() {
                                            self.selected_note = i;
                                            if note.is_image() {
                                                self.viewing_image = Some(note.id.clone());
                                            } else {
                                                open_note_id = Some(note.id.clone());
                                            }
                                        }
                                    }
                                });

                            // ドロップ確定
                            if let Some(payload) = released {
                                let from = *payload;
                                let to = hovered_note
                                    .unwrap_or(self.notes.len().saturating_sub(1));
                                if from < self.notes.len() && from != to {
                                    let item = self.notes.remove(from);
                                    let dest = to.min(self.notes.len());
                                    self.notes.insert(dest, item);
                                    self.renumber_notes();
                                    // 選択もドロップ位置に追従
                                    self.selected_note = dest;
                                    self.dirty_notes = true;
                                }
                            }
                        });
                }
            }
        });

        // ===== ループ後: 時刻ラベルクリック → 日時編集ダイアログ =====
        // タスクの右クリックメニュー由来のアクション（重要度/削除/リマインダー）は
        // draw_context_menu で直接実行されるため、ここでは扱わない。
        if let Some(i) = open_date_idx {
            if let Some(t) = self.tasks.get(i) {
                self.edit_buf = t
                    .scheduled_date_time
                    .map(DateBuf::from_dt)
                    .unwrap_or_else(DateBuf::now);
                self.editing_task_date = Some(t.id.clone());
            }
        }
        if let Some(id) = open_note_id {
            self.editing_note = Some(id);
            self.focus_editor = true;
        }
        // ===== ループ後: タスクをクリック → ミニウィンドウを開く =====
        // 既存ウィンドウがあれば差し替え（ユーザー指示: 既存を閉じて開く）
        if let Some(id) = open_view_task_id {
            let buf = self
                .tasks
                .iter()
                .find(|t| t.id == id)
                .map(|t| t.task_content.clone())
                .unwrap_or_default();
            self.viewing_task_id = Some(id);
            self.viewing_task_edit_mode = false;
            self.viewing_task_edit_buf = buf;
        }

        // ===== タスク日時編集ウィンドウ =====
        if self.editing_task_date.is_some() {
            let mut open = true;
            let mut apply = false;
            let mut clear = false;
            egui::Window::new("予定日時の設定")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        date_fields(ui, &mut self.edit_buf);
                    });
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("設定").clicked() {
                            apply = true;
                        }
                        if ui.button("クリア（未設定に）").clicked() {
                            clear = true;
                        }
                        if ui.button("キャンセル").clicked() {
                            open = false;
                        }
                    });
                });
            if let Some(id) = self.editing_task_date.clone() {
                if let Some(t) = self.tasks.iter_mut().find(|t| t.id == id) {
                    if apply {
                        if let Some(dt) = self.edit_buf.to_dt() {
                            t.scheduled_date_time = Some(dt);
                            t.touch();
                            self.dirty_tasks = true;
                            self.editing_task_date = None;
                        }
                    } else if clear {
                        t.scheduled_date_time = None;
                        t.touch();
                        self.dirty_tasks = true;
                        self.editing_task_date = None;
                    }
                }
            }
            if !open {
                self.editing_task_date = None;
            }
        }

        // ===== メモ編集ウィンドウ（別ビューポート） =====
        if let Some(id) = self.editing_note.clone() {
            if let Some(pos) = self.notes.iter().position(|n| n.id == id) {
                let mut close = false;
                let title = self.notes[pos].title();
                let editor_size = self.settings.editor_size.unwrap_or([520.0, 420.0]);
                let mut builder = egui::ViewportBuilder::default()
                    .with_title(format!("メモの編集 — {title}"))
                    .with_inner_size(editor_size)
                    .with_min_inner_size([260.0, 200.0])
                    .with_window_level(self.sub_window_level());
                if let Some(p) = self.settings.editor_pos {
                    let p = match self.monitor_size {
                        Some(mon) => clamp_pos(p, editor_size, mon),
                        None => p,
                    };
                    builder = builder.with_position(p);
                }
                ctx.show_viewport_immediate(
                    egui::ViewportId::from_hash_of("note_editor"),
                    builder,
                    |ctx2, _class| {
                        ctx2.input(|i| {
                            let vp = i.viewport();
                            if let Some(inner) = vp.inner_rect {
                                self.settings.editor_size = Some([inner.width(), inner.height()]);
                            }
                            if let Some(outer) = vp.outer_rect {
                                self.settings.editor_pos = Some([outer.min.x, outer.min.y]);
                            }
                        });

                        egui::TopBottomPanel::bottom("editor_bottom").show(ctx2, |ui| {
                            ui.add_space(2.0);
                            ui.horizontal(|ui| {
                                let chars = self.notes[pos].task_content.chars().count();
                                ui.weak(format!("{chars} 文字"));
                                ui.weak("｜");
                                ui.weak(format!(
                                    "更新: {}",
                                    fmt_updated(&self.notes[pos].updated_at)
                                ));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui.button("保存").clicked() {
                                            close = true;
                                        }
                                    },
                                );
                            });
                            ui.add_space(2.0);
                        });

                        egui::CentralPanel::default().show(ctx2, |ui| {
                            let note = &mut self.notes[pos];
                            let resp = ui.add_sized(
                                ui.available_size(),
                                egui::TextEdit::multiline(&mut note.task_content)
                                    .hint_text("ここにメモを入力…")
                                    .frame(false),
                            );
                            if resp.changed() {
                                note.touch();
                                self.dirty_notes = true;
                            }
                            if self.focus_editor {
                                resp.request_focus();
                                self.focus_editor = false;
                            }
                        });

                        if ctx2.input(|i| i.viewport().close_requested()) {
                            close = true;
                        }
                    },
                );
                if close {
                    if let Some(p) = self.notes.iter().position(|n| n.id == id) {
                        if self.notes[p].task_content.trim().is_empty() {
                            self.notes.remove(p);
                            self.renumber_notes();
                            if self.selected_note >= self.notes.len() {
                                self.selected_note = self.notes.len().saturating_sub(1);
                            }
                        }
                    }
                    self.editing_note = None;
                    self.persist_notes();
                    self.dirty_notes = false;
                }
            } else {
                self.editing_note = None;
            }
        }

        // ===== 画像ビューアウィンドウ（別ビューポート） =====
        if let Some(id) = self.viewing_image.clone() {
            if let Some(note) = self.notes.iter().find(|n| n.id == id).cloned() {
                if note.is_image() {
                    let mut close = false;
                    let title = note
                        .image_name
                        .clone()
                        .unwrap_or_else(|| "(画像)".into());
                    let viewer_size = self.settings.image_viewer_size.unwrap_or([720.0, 560.0]);
                    let mut builder = egui::ViewportBuilder::default()
                        .with_title(format!("画像 — {title}"))
                        .with_inner_size(viewer_size)
                        .with_min_inner_size([320.0, 240.0])
                        .with_window_level(self.sub_window_level());
                    if let Some(p) = self.settings.image_viewer_pos {
                        let p = match self.monitor_size {
                            Some(mon) => clamp_pos(p, viewer_size, mon),
                            None => p,
                        };
                        builder = builder.with_position(p);
                    }
                    let uri = image_uri(&note);
                    ctx.show_viewport_immediate(
                        egui::ViewportId::from_hash_of("image_viewer"),
                        builder,
                        |ctx2, _class| {
                            ctx2.input(|i| {
                                let vp = i.viewport();
                                if let Some(inner) = vp.inner_rect {
                                    self.settings.image_viewer_size =
                                        Some([inner.width(), inner.height()]);
                                }
                                if let Some(outer) = vp.outer_rect {
                                    self.settings.image_viewer_pos =
                                        Some([outer.min.x, outer.min.y]);
                                }
                            });

                            egui::TopBottomPanel::bottom("viewer_bottom").show(ctx2, |ui| {
                                ui.add_space(2.0);
                                ui.horizontal(|ui| {
                                    ui.weak(format!(
                                        "更新: {}",
                                        fmt_updated(&note.updated_at)
                                    ));
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui.button("閉じる").clicked() {
                                                close = true;
                                            }
                                        },
                                    );
                                });
                                ui.add_space(2.0);
                            });

                            egui::CentralPanel::default().show(ctx2, |ui| {
                                let avail = ui.available_size();
                                // 縦横比を維持して中央に最大表示
                                ui.centered_and_justified(|ui| {
                                    ui.add(
                                        egui::Image::new(uri.clone())
                                            .max_size(avail)
                                            .maintain_aspect_ratio(true),
                                    );
                                });
                            });

                            if ctx2.input(|i| i.viewport().close_requested()) {
                                close = true;
                            }
                        },
                    );
                    if close {
                        self.viewing_image = None;
                        self.dirty_notes = true; // ウィンドウ位置・サイズを保存させる
                    }
                } else {
                    self.viewing_image = None;
                }
            } else {
                self.viewing_image = None;
            }
        }

        // ===== ドラッグ&ドロップ（メモモードのみ） =====
        if self.mode == Mode::Note {
            let dropped = ctx.input(|i| i.raw.dropped_files.clone());
            for f in dropped {
                if let Some(p) = f.path {
                    self.add_image_note(&p);
                }
            }
        }

        // ===== 外部変更の競合ダイアログ =====
        if let Some(target) = self.external_conflict {
            let (file_name, is_task) = match target {
                ConflictTarget::Tasks => ("tasks.json", true),
                ConflictTarget::Notes => ("notes.json", false),
            };
            let mut reload = false;
            let mut keep = false;
            egui::Window::new("外部で変更されました")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(format!(
                        "{file_name} が外部（クラウド同期など）で変更されました。"
                    ));
                    ui.label("このアプリには未保存の変更があります。");
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("再読込（自分の変更を破棄）").clicked() {
                            reload = true;
                        }
                        if ui.button("自分の変更を保持").clicked() {
                            keep = true;
                        }
                    });
                });
            if reload {
                if is_task {
                    self.reload_tasks_from_disk();
                } else {
                    self.editing_note = None;
                    self.reload_notes_from_disk();
                }
                self.external_conflict = None;
            } else if keep {
                if is_task {
                    self.last_tasks_mtime = storage::tasks_mtime();
                } else {
                    self.last_notes_mtime = storage::notes_mtime();
                }
                self.external_conflict = None;
            }
        }

        // ===== 設定ウィンドウ（別ビューポート: メインウィンドウのサイズに左右されない） =====
        if self.show_settings {
            let settings_size = self.settings.settings_size.unwrap_or([380.0, 480.0]);
            let mut builder = egui::ViewportBuilder::default()
                .with_title("⚙ 設定 — UniNote")
                .with_inner_size(settings_size)
                .with_min_inner_size([300.0, 240.0])
                .with_window_level(self.sub_window_level());

            // 初期位置: メインウィンドウ左上 + 保存オフセット。
            // オフセット未保存ならメインウィンドウの中央に重ねる。
            // どちらの場合もモニタ可視範囲にクランプして画面外に出ないようにする。
            let initial_pos: Option<[f32; 2]> = match (self.win_pos, self.monitor_size) {
                (Some(mp), Some(mon)) => {
                    let offset = self.settings.settings_offset.unwrap_or_else(|| {
                        let ms = self.win_size.unwrap_or([0.0, 0.0]);
                        [
                            (ms[0] - settings_size[0]) / 2.0,
                            (ms[1] - settings_size[1]) / 2.0,
                        ]
                    });
                    let candidate = [mp[0] + offset[0], mp[1] + offset[1]];
                    Some(clamp_pos(candidate, settings_size, mon))
                }
                _ => None,
            };
            if let Some(p) = initial_pos {
                builder = builder.with_position(p);
            }
            let mut close = false;
            ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of("settings_window"),
                builder,
                |ctx2, _class| {
                    // 位置はメインウィンドウ左上からのオフセットとして記憶。
                    // サイズはそのまま絶対値で記憶。
                    ctx2.input(|i| {
                        let vp = i.viewport();
                        if let Some(inner) = vp.inner_rect {
                            self.settings.settings_size =
                                Some([inner.width(), inner.height()]);
                        }
                        if let Some(outer) = vp.outer_rect {
                            if let Some(mp) = self.win_pos {
                                self.settings.settings_offset =
                                    Some([outer.min.x - mp[0], outer.min.y - mp[1]]);
                            }
                        }
                    });

                    egui::CentralPanel::default().show(ctx2, |ui| {
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                    egui::Grid::new("settings_grid")
                        .num_columns(2)
                        .spacing([12.0, 8.0])
                        .show(ui, |ui| {
                            // フォント
                            ui.label("フォント");
                            egui::ComboBox::from_id_salt("font_family")
                                .selected_text(self.settings.font_family.clone())
                                .show_ui(ui, |ui| {
                                    for (name, _) in available_fonts() {
                                        if ui
                                            .selectable_value(
                                                &mut self.settings.font_family,
                                                name.to_string(),
                                                name,
                                            )
                                            .changed()
                                        {
                                            self.dirty_tasks = true;
                                        }
                                    }
                                });
                            ui.end_row();

                            // 文字サイズ
                            ui.label("文字サイズ");
                            if ui
                                .add(egui::Slider::new(
                                    &mut self.settings.font_size,
                                    10.0..=32.0,
                                ))
                                .changed()
                            {
                                self.dirty_tasks = true;
                            }
                            ui.end_row();

                            // テーマ
                            ui.label("テーマ");
                            egui::ComboBox::from_id_salt("theme")
                                .selected_text(self.settings.theme.label())
                                .show_ui(ui, |ui| {
                                    for t in
                                        [ThemeMode::System, ThemeMode::Dark, ThemeMode::Light]
                                    {
                                        if ui
                                            .selectable_value(
                                                &mut self.settings.theme,
                                                t,
                                                t.label(),
                                            )
                                            .changed()
                                        {
                                            self.dirty_tasks = true;
                                        }
                                    }
                                });
                            ui.end_row();

                            // 常に最前面
                            ui.label("表示");
                            if ui
                                .checkbox(&mut self.settings.always_on_top, "常に最前面に表示")
                                .changed()
                            {
                                self.dirty_tasks = true;
                            }
                            ui.end_row();

                            // 外部連携セットアップウィザード起動
                            ui.label("外部連携");
                            if ui.button("🔗 外部連携の設定…").clicked() {
                                self.show_setup_wizard = true;
                            }
                            ui.end_row();

                            // ─ 自動同期 ─
                            ui.label("自動同期");
                            ui.horizontal(|ui| {
                                if ui
                                    .checkbox(
                                        &mut self.settings.auto_sync_on_startup,
                                        "起動時に実行",
                                    )
                                    .changed()
                                {
                                    self.dirty_tasks = true;
                                }
                                ui.add_space(8.0);
                                ui.label("間隔(分):");
                                if ui
                                    .add(
                                        egui::DragValue::new(
                                            &mut self.settings.auto_sync_interval_min,
                                        )
                                        .range(0..=1440)
                                        .speed(1.0)
                                        .custom_formatter(|n, _| {
                                            if n <= 0.0 {
                                                "off".into()
                                            } else {
                                                format!("{}", n as u32)
                                            }
                                        }),
                                    )
                                    .changed()
                                {
                                    self.dirty_tasks = true;
                                }
                                ui.weak("（0で定期同期オフ）");
                            });
                            ui.end_row();

                            // ─ 関連ツール（プラグイン）検出状況 ─
                            // 起動はタスクモードのツールバーから行う。ここは状況表示のみ。
                            ui.label("関連ツール");
                            ui.vertical(|ui| {
                                // 検出状況の表示（元の見た目を維持）
                                let known: &[(&str, &str)] = &[
                                    ("simplecalendar", "📅 SimpleCalendar"),
                                    ("pc-reminder-gui", "⏰ PCReminder"),
                                ];
                                for (key, label) in known {
                                    let installed = self.is_tool_available(key);
                                    let (mark, color) = if installed {
                                        ("✅ 検出済", Color32::from_rgb(120, 200, 120))
                                    } else {
                                        ("— 未検出", Color32::from_rgb(170, 170, 170))
                                    };
                                    ui.horizontal(|ui| {
                                        ui.label(*label);
                                        ui.colored_label(color, mark);
                                    });
                                }
                                ui.add_space(2.0);
                                // リスト再スキャン
                                if ui.small_button("🔄 リスト再スキャン").clicked() {
                                    self.tools = storage::discover_tools(
                                        &self.settings.manual_tool_paths,
                                    );
                                }
                                // 「登録(検出されない時)」ボタン: 常時表示。
                                // 選んだ exe のファイル名から既知ツールを自動判定して登録する。
                                // 検出済ツールでも、ここから選び直せばパスを上書き登録できる。
                                let entries = storage::known_tool_entries();
                                let mut do_register = false;
                                if ui.small_button("登録(検出されない時)").clicked() {
                                    do_register = true;
                                }
                                if do_register {
                                    if let Some(picked) = rfd::FileDialog::new()
                                        .add_filter("実行ファイル", &["exe"])
                                        .set_title("未検出ツールの exe を選択")
                                        .pick_file()
                                    {
                                        let picked_name = picked
                                            .file_name()
                                            .map(|n| n.to_string_lossy().to_ascii_lowercase())
                                            .unwrap_or_default();
                                        // ファイル名で既知ツールを自動判定
                                        let matched = entries.iter().find(|(_, _, fname)| {
                                            fname.to_ascii_lowercase() == picked_name
                                        });
                                        match matched {
                                            Some((key, label, _)) => {
                                                self.settings.manual_tool_paths.insert(
                                                    key.clone(),
                                                    picked.to_string_lossy().to_string(),
                                                );
                                                self.tools = storage::discover_tools(
                                                    &self.settings.manual_tool_paths,
                                                );
                                                self.tool_status = Some(format!(
                                                    "✅ {} を登録しました",
                                                    label
                                                ));
                                                self.dirty_tasks = true;
                                            }
                                            None => {
                                                self.tool_status = Some(format!(
                                                    "⚠ 既知のツール名と一致しません ({})",
                                                    picked_name
                                                ));
                                            }
                                        }
                                    }
                                }
                                if let Some(msg) = &self.tool_status {
                                    ui.add_space(2.0);
                                    ui.weak(msg);
                                }
                            });
                            ui.end_row();

                            // ─ メモ設定 ─
                            ui.separator();
                            ui.label("── メモ設定 ──");
                            ui.end_row();

                            // 画像メモ表示形式
                            ui.label("画像メモ表示");
                            ui.horizontal(|ui| {
                                if ui
                                    .selectable_value(
                                        &mut self.settings.image_display,
                                        ImageDisplay::FileName,
                                        ImageDisplay::FileName.label(),
                                    )
                                    .changed()
                                {
                                    self.dirty_notes = true;
                                }
                                if ui
                                    .selectable_value(
                                        &mut self.settings.image_display,
                                        ImageDisplay::Thumbnail,
                                        ImageDisplay::Thumbnail.label(),
                                    )
                                    .changed()
                                {
                                    self.dirty_notes = true;
                                }
                            });
                            ui.end_row();

                            let thumb_enabled =
                                self.settings.image_display == ImageDisplay::Thumbnail;

                            // サムネ形状
                            ui.label("サムネ形状");
                            ui.add_enabled_ui(thumb_enabled, |ui| {
                                ui.horizontal(|ui| {
                                    if ui
                                        .selectable_value(
                                            &mut self.settings.thumb_shape,
                                            ThumbShape::Crop,
                                            ThumbShape::Crop.label(),
                                        )
                                        .changed()
                                    {
                                        self.dirty_notes = true;
                                    }
                                    if ui
                                        .selectable_value(
                                            &mut self.settings.thumb_shape,
                                            ThumbShape::Stretch,
                                            ThumbShape::Stretch.label(),
                                        )
                                        .changed()
                                    {
                                        self.dirty_notes = true;
                                    }
                                });
                            });
                            ui.end_row();

                            // サムネ高さ
                            ui.label("サムネ高さ");
                            ui.add_enabled_ui(thumb_enabled, |ui| {
                                if ui
                                    .add(
                                        egui::Slider::new(
                                            &mut self.settings.thumb_height,
                                            32.0..=96.0,
                                        )
                                        .suffix(" px"),
                                    )
                                    .changed()
                                {
                                    self.dirty_notes = true;
                                }
                            });
                            ui.end_row();

                            // ─ タスク設定 ─
                            ui.separator();
                            ui.label("── タスク設定 ──");
                            ui.end_row();

                            // タスク登録キー
                            ui.label("タスク登録キー")
                                .on_hover_text(
                                    "タスク入力欄でタスクを登録するキー。\n誤登録を防ぐため既定は「なし」（追加ボタンのみ）。",
                                );
                            egui::ComboBox::from_id_salt("add_task_key")
                                .selected_text(self.settings.add_task_key.label())
                                .show_ui(ui, |ui| {
                                    for k in [
                                        AddTaskKey::None,
                                        AddTaskKey::Enter,
                                        AddTaskKey::ShiftEnter,
                                        AddTaskKey::CtrlEnter,
                                    ] {
                                        if ui
                                            .selectable_value(
                                                &mut self.settings.add_task_key,
                                                k,
                                                k.label(),
                                            )
                                            .changed()
                                        {
                                            self.dirty_tasks = true;
                                        }
                                    }
                                });
                            ui.end_row();

                            // PC操作の初期値（起動時と、タスクが追加されたタイミングで適用される値）
                            // PCReminder 検出時のみ表示。
                            if self.is_tool_available("pc-reminder-gui") {
                                ui.label("PC操作の初期値").on_hover_text(
                                    "「PC操作」ボタンが既定で選ぶ値。\n\
                                     アプリ起動時、およびタスクが追加されたら（追加ボタン・ショートカットいずれの場合も）\nこの値にリセットされる。",
                                );
                                egui::ComboBox::from_id_salt("pc_action_default")
                                    .selected_text(self.settings.pc_action_default.label())
                                    .show_ui(ui, |ui| {
                                        for a in [
                                            PcAction::None,
                                            PcAction::Notify,
                                            PcAction::Wake,
                                            PcAction::Sleep,
                                        ] {
                                            if ui
                                                .selectable_value(
                                                    &mut self.settings.pc_action_default,
                                                    a,
                                                    a.label(),
                                                )
                                                .changed()
                                            {
                                                // 起動中なら現在の new_task_pc_action も同期
                                                self.new_task_pc_action = a;
                                                self.dirty_tasks = true;
                                            }
                                        }
                                    });
                                ui.end_row();
                            }

                            // 重要度カラー
                            ui.label("重要度カラー");
                            ui.horizontal(|ui| {
                                let c = &mut self.settings.colors;
                                let mut changed = false;
                                for (lbl, arr) in [
                                    ("高", &mut c.high),
                                    ("中", &mut c.medium),
                                    ("低", &mut c.low),
                                    ("なし", &mut c.none),
                                ] {
                                    ui.label(lbl);
                                    let mut col = color_of(*arr);
                                    if ui.color_edit_button_srgba(&mut col).changed() {
                                        *arr = [col.r(), col.g(), col.b(), col.a()];
                                        changed = true;
                                    }
                                }
                                if changed {
                                    self.dirty_tasks = true;
                                }
                            });
                            ui.end_row();

                            // 日時表示（24h以上）
                            ui.label("表示(24時間以上)");
                            ui.horizontal(|ui| {
                                if ui
                                    .selectable_value(
                                        &mut self.settings.far_format,
                                        FarFormat::Date,
                                        "日付 (06/20)",
                                    )
                                    .changed()
                                {
                                    self.dirty_tasks = true;
                                }
                                if ui
                                    .selectable_value(
                                        &mut self.settings.far_format,
                                        FarFormat::RelativeDays,
                                        "残り日数",
                                    )
                                    .changed()
                                {
                                    self.dirty_tasks = true;
                                }
                            });
                            ui.end_row();

                            // 日時表示（24h未満）
                            ui.label("表示(24時間未満)");
                            ui.horizontal(|ui| {
                                if ui
                                    .selectable_value(
                                        &mut self.settings.near_format,
                                        NearFormat::Time,
                                        "時刻 (15:00)",
                                    )
                                    .changed()
                                {
                                    self.dirty_tasks = true;
                                }
                                if ui
                                    .selectable_value(
                                        &mut self.settings.near_format,
                                        NearFormat::RelativeHours,
                                        "残り時間",
                                    )
                                    .changed()
                                {
                                    self.dirty_tasks = true;
                                }
                            });
                            ui.end_row();
                        }); // Grid
                            }); // ScrollArea
                    }); // CentralPanel

                    if ctx2.input(|i| i.viewport().close_requested()) {
                        close = true;
                    }
                },
            );
            if close {
                self.show_settings = false;
                // 設定ウィンドウ位置・サイズの保存
                self.dirty_tasks = true;
            }
        }

        // ===== タスク右クリックメニュー（別ビューポート） =====
        if self.context_menu_task_id.is_some() {
            self.draw_context_menu(ctx);
        }

        // ===== タスク内容ビューワ（別ビューポート） =====
        if self.viewing_task_id.is_some() {
            self.draw_task_viewer(ctx);
        }

        // ===== ホバープレビュー（別 OS ウィンドウ。タスク本文を右/左側に表示） =====
        self.draw_hover_preview(ctx);

        // ===== 外部連携セットアップウィザード（別ビューポート） =====
        if self.show_setup_wizard {
            self.draw_setup_wizard(ctx);
        }

        // ウィザード内サイドカープロセスのメッセージドレイン
        if let Some(active) = self.wizard_active.as_mut() {
            while let Ok(msg) = active.rx.try_recv() {
                match msg {
                    SyncMsg::Line(l) => active.log.push(l),
                    SyncMsg::Done(c) => {
                        if active.exit_code.is_none() {
                            active.exit_code = Some(c);
                            active.finished_at = Some(Instant::now());
                            // サイドカーリストを再スキャン（新しく認可されたものを反映）
                            self.sidecars = storage::discover_sidecars();
                        }
                    }
                }
            }
            if active.exit_code.is_none() {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }

        // ===== サイドカー: メッセージ吸い上げ → 完了で再読込 =====
        let mut sync_just_finished = false;
        if let Some(active) = self.active_sync.as_mut() {
            while let Ok(msg) = active.rx.try_recv() {
                match msg {
                    SyncMsg::Line(l) => active.log.push(l),
                    SyncMsg::Done(c) => {
                        if active.exit_code.is_none() {
                            active.exit_code = Some(c);
                            active.finished_at = Some(Instant::now());
                            sync_just_finished = true;
                        }
                    }
                }
            }
            if active.exit_code.is_none() {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
        if sync_just_finished {
            self.reload_after_sync();
            // 手動・自動を問わず last_auto_sync_at をリセット
            // （手動同期した直後は定期同期を待たせる）
            self.last_auto_sync_at = Instant::now();
            if self.current_sync_is_auto {
                // 自動同期: 失敗時のみバナー、成功時は無音
                if let Some(active) = &self.active_sync {
                    let code = active.exit_code.unwrap_or(-1);
                    if code != 0 {
                        self.status_msg = Some(format!(
                            "⚠ {} の自動同期が失敗 (exit={code})。手動同期で詳細ログを確認してください",
                            active.name
                        ));
                    }
                }
                self.active_sync = None;
                self.current_sync_is_auto = false;
            }
        }

        // ===== 自動同期スケジューラ =====
        // active_sync が無い時にキューを処理 → 空なら起動条件をチェックして詰める。
        // ビューワが編集モードの間は未保存編集の取りこぼし防止のため一時停止する
        // （手動同期と日時編集ダイアログには影響しない）。
        let pause_auto_sync = self.viewing_task_edit_mode;
        if self.active_sync.is_none() && !pause_auto_sync {
            if let Some(sc) = self.auto_sync_queue.pop_front() {
                self.start_sync(&sc, true);
            } else {
                let need_startup =
                    !self.startup_sync_done && self.settings.auto_sync_on_startup;
                let interval_due = self.settings.auto_sync_interval_min > 0
                    && self.last_auto_sync_at.elapsed()
                        >= Duration::from_secs(
                            self.settings.auto_sync_interval_min as u64 * 60,
                        );
                if (need_startup || interval_due) && !self.sidecars.is_empty() {
                    for sc in &self.sidecars {
                        self.auto_sync_queue.push_back(sc.clone());
                    }
                    self.startup_sync_done = true;
                    // 次回トリガーまでの待機時刻をリセット
                    self.last_auto_sync_at = Instant::now();
                }
            }
        }

        // 自動同期が有効な間は定期的に repaint してスケジューラを動かす
        if self.settings.auto_sync_interval_min > 0
            || !self.auto_sync_queue.is_empty()
        {
            ctx.request_repaint_after(Duration::from_secs(30));
        }

        // ===== サイドカー同期ログウィンドウ =====
        if self.show_sync_log && self.active_sync.is_some() {
            let mut close = false;
            let mut clear_after = false;
            let (name, log, exit_code, started, finished) = {
                let a = self.active_sync.as_ref().unwrap();
                (
                    a.name.clone(),
                    a.log.clone(),
                    a.exit_code,
                    a.started_at,
                    a.finished_at,
                )
            };
            egui::Window::new(format!("🔄 同期: {name}"))
                .resizable(true)
                .default_size([560.0, 380.0])
                .show(ctx, |ui| {
                    let elapsed = finished
                        .map(|f| f.duration_since(started))
                        .unwrap_or_else(|| started.elapsed());
                    ui.horizontal(|ui| {
                        match exit_code {
                            None => {
                                ui.spinner();
                                ui.label(format!("実行中… {:.1}秒", elapsed.as_secs_f32()));
                            }
                            Some(0) => {
                                ui.label(format!(
                                    "✅ 完了（{:.1}秒）",
                                    elapsed.as_secs_f32()
                                ));
                            }
                            Some(c) => {
                                ui.colored_label(
                                    Color32::from_rgb(220, 100, 100),
                                    format!("❌ 失敗 (exit {c}, {:.1}秒)", elapsed.as_secs_f32()),
                                );
                            }
                        }
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if ui.button("閉じる").clicked() {
                                    close = true;
                                    if exit_code.is_some() {
                                        clear_after = true;
                                    }
                                }
                            },
                        );
                    });
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            if log.is_empty() {
                                ui.weak("（出力なし）");
                            } else {
                                for line in &log {
                                    ui.label(RichText::new(line).monospace().small());
                                }
                            }
                        });
                });
            if close {
                self.show_sync_log = false;
                if clear_after {
                    self.active_sync = None;
                }
            }
        }

        // ===== フォント変更の反映 =====
        if self.settings.font_family != self.current_font {
            apply_font(ctx, &self.settings.font_family);
            self.current_font = self.settings.font_family.clone();
        }

        // ===== 終了 / 自動保存 =====
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.dirty_tasks {
                self.persist_tasks();
            }
            if self.dirty_notes {
                self.persist_notes();
            }
            // tasks も notes も dirty でない場合もウィンドウ位置を保存
            if !self.dirty_tasks && !self.dirty_notes {
                self.settings.window_pos = self.win_pos;
                self.settings.window_size = self.win_size;
                storage::save_settings(&self.settings);
            }
        } else {
            // タスクのデバウンス自動保存
            if self.dirty_tasks && self.external_conflict != Some(ConflictTarget::Tasks) {
                let elapsed = self.last_saved_tasks.elapsed();
                if elapsed >= AUTOSAVE_DEBOUNCE {
                    self.persist_tasks();
                    self.dirty_tasks = false;
                } else {
                    ctx.request_repaint_after(AUTOSAVE_DEBOUNCE - elapsed);
                }
            }
            // メモのデバウンス自動保存
            if self.dirty_notes && self.external_conflict != Some(ConflictTarget::Notes) {
                let elapsed = self.last_saved_notes.elapsed();
                if elapsed >= AUTOSAVE_DEBOUNCE {
                    self.persist_notes();
                    self.dirty_notes = false;
                } else {
                    ctx.request_repaint_after(AUTOSAVE_DEBOUNCE - elapsed);
                }
            }
        }
    }
}
