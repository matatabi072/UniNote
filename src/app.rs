use crate::model::{Item, Priority};
use crate::settings::{
    available_fonts, font_path_for, FarFormat, ImageDisplay, NearFormat, Settings, SortMode,
    ThemeMode, ThumbShape,
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

fn date_fields(ui: &mut egui::Ui, b: &mut DateBuf) {
    let h = ui.spacing().interact_size.y;
    let wy = 44.0;
    let wn = 22.0;
    ui.spacing_mut().item_spacing.x = 2.0;
    ui.add_sized([wy, h], egui::DragValue::new(&mut b.y).speed(1));
    ui.label("/");
    ui.add_sized([wn, h], egui::DragValue::new(&mut b.mo).speed(1));
    ui.label("/");
    ui.add_sized([wn, h], egui::DragValue::new(&mut b.d).speed(1));
    ui.add_sized([wn, h], egui::DragValue::new(&mut b.h).speed(1));
    ui.label(":");
    ui.add_sized([wn, h], egui::DragValue::new(&mut b.mi).speed(1));
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
        let status_msg = tasks_msg.or(notes_msg);
        Self {
            mode: Mode::Task,
            tasks,
            notes,
            settings,
            new_task_text: String::new(),
            new_task_use_date: false,
            new_task_buf: DateBuf::now(),
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

    /// メインウィンドウが常に最前面の場合、サブウィンドウも同じレベルにする。
    /// そうしないと AlwaysOnTop のメインに隠れて操作できなくなる。
    fn sub_window_level(&self) -> egui::WindowLevel {
        if self.settings.always_on_top {
            egui::WindowLevel::AlwaysOnTop
        } else {
            egui::WindowLevel::Normal
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

    /// サイドカーを起動して標準出力をログに流す
    fn start_sync(&mut self, sc: &storage::SidecarInfo) {
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
                self.show_sync_log = true;
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
                self.show_sync_log = true;
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
        self.tasks.push(task);
        self.new_task_text.clear();
        self.dirty_tasks = true;
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
}

fn row_menu(ui: &mut egui::Ui) -> Option<RowAction> {
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

#[cfg(windows)]
fn open_url(url: &str) {
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
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
                                self.start_sync(&sc);
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
                    ui.horizontal_wrapped(|ui| {
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut self.new_task_text)
                                .hint_text("新しいタスクを入力して Enter")
                                .desired_width(200.0),
                        );
                        let enter = resp.lost_focus()
                            && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if ui.button("追加").clicked() || enter {
                            self.add_task();
                            resp.request_focus();
                        }
                        ui.label("  並び:");
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
                    ui.horizontal_wrapped(|ui| {
                        ui.checkbox(&mut self.new_task_use_date, "予定日時");
                        let use_date = self.new_task_use_date;
                        ui.add_enabled_ui(use_date, |ui| {
                            date_fields(ui, &mut self.new_task_buf);
                            if ui.small_button("今すぐ").clicked() {
                                self.new_task_buf = DateBuf::now();
                            }
                        });
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

        // ===== メインコンテンツ =====
        let now = Local::now().naive_local();
        let far = self.settings.far_format;
        let near = self.settings.near_format;
        let colors = self.settings.colors;
        let manual_mode = self.settings.sort_mode == SortMode::Manual;
        let order = self.task_display_order();

        // タスクリスト用の操作バッファ
        let mut set_priority: Option<(usize, Priority)> = None;
        let mut delete_task_idx: Option<usize> = None;
        let mut open_date_idx: Option<usize> = None;
        let mut hovered_row: Option<usize> = None;
        // メモリスト用
        let mut open_note_id: Option<String> = None;

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
                                                            if task.is_completed {
                                                                ui.add(
                                                                    egui::Label::new(
                                                                        RichText::new(
                                                                            &task.task_content,
                                                                        )
                                                                        .strikethrough()
                                                                        .weak(),
                                                                    )
                                                                    .truncate(),
                                                                );
                                                            } else {
                                                                let r = ui.add(
                                                                    egui::TextEdit::singleline(
                                                                        &mut task.task_content,
                                                                    )
                                                                    .frame(false)
                                                                    .desired_width(f32::INFINITY),
                                                                );
                                                                if r.changed() {
                                                                    task.touch();
                                                                    *dirty = true;
                                                                }
                                                            }
                                                        },
                                                    );
                                                });
                                            });
                                        let row_resp = fr.response;
                                        if row_resp.contains_pointer() {
                                            hovered_row = Some(i);
                                        }
                                        let popup_id =
                                            ui.make_persistent_id(("ctxmenu", &task_id));
                                        if row_resp.contains_pointer()
                                            && ui.input(|inp| inp.pointer.secondary_clicked())
                                        {
                                            ui.memory_mut(|m| m.open_popup(popup_id));
                                        }
                                        let action = egui::popup_below_widget(
                                            ui,
                                            popup_id,
                                            &row_resp,
                                            egui::PopupCloseBehavior::CloseOnClick,
                                            |ui| row_menu(ui),
                                        )
                                        .flatten();
                                        if let Some(a) = action {
                                            match a {
                                                RowAction::SetPriority(p) => {
                                                    set_priority = Some((i, p))
                                                }
                                                RowAction::EditDate => open_date_idx = Some(i),
                                                RowAction::Delete => delete_task_idx = Some(i),
                                            }
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

        // ===== ループ後: タスク操作適用 =====
        if let Some((i, p)) = set_priority {
            if let Some(t) = self.tasks.get_mut(i) {
                t.priority = p;
                t.touch();
                self.dirty_tasks = true;
            }
        }
        if let Some(i) = delete_task_idx {
            if i < self.tasks.len() {
                self.tasks.remove(i);
                self.renumber_tasks();
                self.dirty_tasks = true;
            }
        }
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
