//! Todoist ↔ UniNote 双方向同期（v2）。
//!
//! 3-way merge: 前回同期スナップショット（state.json）を基準に
//! ローカル変更 / リモート変更 / 両方変更 を判別する。
//!
//! ポリシー:
//! - 競合（両方変更）: REMOTE WINS（Todoist 側を正本扱い、ローカル変更は警告ログに残す）
//! - リモートから消えたタスクのローカル削除: しない（誤削除回避）
//! - ローカルから消えたタスクのリモート削除: しない（誤削除回避）
//! - 初回 v2 移行（state なし + 既存 todoist_id あり）: 現状をベースラインに採用
use crate::model::{Item, Priority};
use crate::state::{self, SyncState, TaskSnapshot};
use crate::todoist::{self, TaskMutation, TodoistDue, TodoistTask};
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

const TASKS_FILE: &str = "tasks.json";
const TASKS_BACKUP: &str = "tasks.backup.json";
const TASKS_TMP: &str = "tasks.json.tmp";

#[derive(Default)]
pub struct SyncReport {
    pub fetched: usize,
    pub added_from_remote: usize,
    pub pulled_updates: usize,
    pub pushed_creates: usize,
    pub pushed_updates: usize,
    pub pushed_deletes: usize,
    pub conflicts_remote_wins: usize,
    pub completion_pushed: usize,
    pub unchanged: usize,
    pub errors: usize,
}

pub fn run(
    client: &todoist::Client,
    remote: &[TodoistTask],
    dry_run: bool,
) -> Result<SyncReport, String> {
    let tasks_path = PathBuf::from(TASKS_FILE);
    let mut locals: Vec<Item> = if tasks_path.exists() {
        let raw = fs::read_to_string(&tasks_path)
            .map_err(|e| format!("{TASKS_FILE} 読込失敗: {e}"))?;
        serde_json::from_str(&raw).map_err(|e| format!("{TASKS_FILE} 解析失敗: {e}"))?
    } else {
        Vec::new()
    };

    let mut state = state::load();
    let mut new_state = SyncState {
        version: 1,
        last_sync_at: Some(now_rfc3339()),
        synced_tasks: BTreeMap::new(),
    };

    // 初回 v2 移行: state が空で既存のリモート連携タスクがある → 現状をベースラインに
    if state.synced_tasks.is_empty()
        && locals.iter().any(|l| l.todoist_id.is_some())
    {
        for l in locals.iter() {
            if let Some(tid) = &l.todoist_id {
                state.synced_tasks.insert(tid.clone(), snapshot_of_local(l));
            }
        }
        println!(
            "[info] 初回 v2 同期: ローカル {} 件をベースラインに記録",
            state.synced_tasks.len()
        );
    }

    let mut report = SyncReport {
        fetched: remote.len(),
        ..Default::default()
    };
    let mut tasks_changed = false;

    let remote_by_id: BTreeMap<String, &TodoistTask> =
        remote.iter().map(|t| (t.id.clone(), t)).collect();

    // ── Phase 0: ローカル削除を検出 → Todoist 側にも削除を push ──
    // state に居るが現在のローカルには居ない todoist_id = ユーザーが削除した
    let local_tids_now: HashSet<String> =
        locals.iter().filter_map(|t| t.todoist_id.clone()).collect();
    let deleted_locally: Vec<String> = state
        .synced_tasks
        .keys()
        .filter(|tid| !local_tids_now.contains(*tid))
        .cloned()
        .collect();
    // 削除したものは新 state に入れない（自然消滅）
    // また Phase B で「remote にあって local にない」として再追加されないよう just_deleted で除外する
    let mut just_deleted: HashSet<String> = HashSet::new();
    for tid in &deleted_locally {
        if dry_run {
            println!("[dry] push 削除: todoist_id={tid}");
            just_deleted.insert(tid.clone());
            continue;
        }
        match client.delete_task(tid) {
            Ok(()) => {
                println!("[sync] push 削除: todoist_id={tid}");
                report.pushed_deletes += 1;
                just_deleted.insert(tid.clone());
            }
            Err(e) => {
                eprintln!("push 削除失敗 [{tid}]: {e}");
                report.errors += 1;
                // 失敗した場合は state を維持して次回再試行
                if let Some(s) = state.synced_tasks.get(tid).cloned() {
                    new_state.synced_tasks.insert(tid.clone(), s);
                }
            }
        }
    }

    // ── Phase A: 既存ペア（todoist_id を持つローカル）を 3-way merge ──
    for local in locals.iter_mut() {
        let Some(tid) = local.todoist_id.clone() else {
            continue;
        };
        let prev = state.synced_tasks.get(&tid).cloned();
        match remote_by_id.get(tid.as_str()) {
            Some(r) => {
                let local_changed = match &prev {
                    Some(s) => differs_local(local, s),
                    None => false,
                };
                let remote_changed = match &prev {
                    Some(s) => differs_remote(r, s),
                    None => true,
                };
                if local_changed && remote_changed {
                    println!(
                        "[warn] 競合(remote勝ち): {} / ローカル変更を破棄",
                        short(&local.task_content)
                    );
                    apply_remote(local, r);
                    report.conflicts_remote_wins += 1;
                    tasks_changed = true;
                } else if remote_changed {
                    apply_remote(local, r);
                    report.pulled_updates += 1;
                    tasks_changed = true;
                } else if local_changed {
                    if dry_run {
                        println!("[dry] push 更新: {}", short(&local.task_content));
                    } else {
                        match push_local_to_remote(client, r, local) {
                            Ok(comp_changed) => {
                                report.pushed_updates += 1;
                                if comp_changed {
                                    report.completion_pushed += 1;
                                }
                                local.touch();
                                tasks_changed = true;
                            }
                            Err(e) => {
                                eprintln!("push 更新失敗 [{}]: {e}", short(&local.task_content));
                                report.errors += 1;
                            }
                        }
                    }
                } else {
                    report.unchanged += 1;
                }
                new_state
                    .synced_tasks
                    .insert(tid, snapshot_of_local(local));
            }
            None => {
                // リモートから消えた → 削除しない（v2 ポリシー）。state は保持。
                if let Some(s) = prev {
                    new_state.synced_tasks.insert(tid, s);
                }
            }
        }
    }

    // ── Phase B: ローカルにない remote → 追加 ──
    // 今回 push 削除した ID は API がまだ反映してない可能性があるので、再追加しない
    let local_tids: HashSet<String> =
        locals.iter().filter_map(|t| t.todoist_id.clone()).collect();
    for r in remote {
        if local_tids.contains(&r.id) || just_deleted.contains(&r.id) {
            continue;
        }
        let item = item_from_todoist(r, next_order(&locals));
        new_state
            .synced_tasks
            .insert(r.id.clone(), snapshot_of_local(&item));
        locals.push(item);
        report.added_from_remote += 1;
        tasks_changed = true;
    }

    // ── Phase C: 純ローカル（todoist_id なし）→ Todoist に CREATE ──
    let to_create_indices: Vec<usize> = locals
        .iter()
        .enumerate()
        .filter(|(_, l)| l.todoist_id.is_none() && !l.is_completed)
        .map(|(i, _)| i)
        .collect();
    for i in to_create_indices {
        if dry_run {
            println!("[dry] push 新規: {}", short(&locals[i].task_content));
            continue;
        }
        let body = build_mutation(&locals[i]);
        match client.create_task(&body) {
            Ok(created) => {
                println!(
                    "[sync] push 新規: {} → todoist_id={}",
                    short(&locals[i].task_content),
                    created.id
                );
                locals[i].todoist_id = Some(created.id.clone());
                locals[i].touch();
                new_state
                    .synced_tasks
                    .insert(created.id, snapshot_of_local(&locals[i]));
                report.pushed_creates += 1;
                tasks_changed = true;
            }
            Err(e) => {
                eprintln!("push 新規失敗 [{}]: {e}", short(&locals[i].task_content));
                report.errors += 1;
            }
        }
    }

    // ── Phase D: 状態とローカルを書き戻し ──
    if !dry_run {
        if let Err(e) = state::save(&new_state) {
            eprintln!("state 保存失敗: {e}");
        }
        if tasks_changed {
            write_atomic(&tasks_path, &locals)?;
        }
    }

    Ok(report)
}

// ─── 比較 ───

fn differs_local(local: &Item, snap: &TaskSnapshot) -> bool {
    local.task_content != snap.content
        || local.is_completed != snap.is_completed
        || priority_str(local.priority) != snap.priority
        || local.scheduled_date_time != snap.scheduled_date_time
}

fn differs_remote(remote: &TodoistTask, snap: &TaskSnapshot) -> bool {
    remote.content != snap.content
        || remote.is_completed != snap.is_completed
        || priority_str_from_todoist(remote.priority) != snap.priority
        || parse_due(remote.due.as_ref()) != snap.scheduled_date_time
}

// ─── 適用 ───

fn apply_remote(local: &mut Item, r: &TodoistTask) {
    local.task_content = r.content.clone();
    local.is_completed = r.is_completed;
    local.priority = priority_from_todoist(r.priority);
    local.scheduled_date_time = parse_due(r.due.as_ref());
    if local.todoist_id.as_deref() != Some(r.id.as_str()) {
        local.todoist_id = Some(r.id.clone());
    }
    local.touch();
}

/// ローカルの変更を Todoist に push。完了状態の変化（close/reopen）も処理する。
/// 戻り値: 完了状態が変更されたか
fn push_local_to_remote(
    client: &todoist::Client,
    remote: &TodoistTask,
    local: &Item,
) -> Result<bool, String> {
    let completion_changed = local.is_completed != remote.is_completed;
    if completion_changed {
        if local.is_completed {
            client.close_task(&remote.id)?;
        } else {
            client.reopen_task(&remote.id)?;
        }
    }
    let body = build_mutation(local);
    let remote_body = build_mutation_from_remote(remote);
    if body.content != remote_body.content
        || body.priority != remote_body.priority
        || body.due_datetime != remote_body.due_datetime
        || body.due_date != remote_body.due_date
    {
        client.update_task(&remote.id, &body)?;
    }
    Ok(completion_changed)
}

// ─── 変換 ───

fn build_mutation(local: &Item) -> TaskMutation {
    let priority = priority_to_todoist(local.priority);
    let (due_datetime, due_date) = match local.scheduled_date_time {
        Some(dt) => match Local.from_local_datetime(&dt).single() {
            Some(local_dt) => {
                let utc = local_dt.with_timezone(&Utc);
                (
                    Some(utc.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
                    None,
                )
            }
            None => (None, None),
        },
        None => (None, None),
    };
    TaskMutation {
        content: local.task_content.clone(),
        priority,
        due_datetime,
        due_date,
    }
}

fn build_mutation_from_remote(r: &TodoistTask) -> TaskMutation {
    let (due_datetime, due_date) = match parse_due(r.due.as_ref()) {
        Some(dt) => match Local.from_local_datetime(&dt).single() {
            Some(local_dt) => {
                let utc = local_dt.with_timezone(&Utc);
                (
                    Some(utc.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
                    None,
                )
            }
            None => (None, None),
        },
        None => (None, None),
    };
    TaskMutation {
        content: r.content.clone(),
        priority: r.priority,
        due_datetime,
        due_date,
    }
}

fn item_from_todoist(r: &TodoistTask, order: i64) -> Item {
    Item {
        id: uuid::Uuid::new_v4().to_string(),
        google_task_id: None,
        task_content: r.content.clone(),
        is_completed: r.is_completed,
        scheduled_date_time: parse_due(r.due.as_ref()),
        priority: priority_from_todoist(r.priority),
        manual_order: order,
        updated_at: now_rfc3339(),
        kind: "text".to_string(),
        image_name: None,
        todoist_id: Some(r.id.clone()),
        extra: Default::default(),
    }
}

fn snapshot_of_local(l: &Item) -> TaskSnapshot {
    TaskSnapshot {
        content: l.task_content.clone(),
        is_completed: l.is_completed,
        priority: priority_str(l.priority),
        scheduled_date_time: l.scheduled_date_time,
    }
}

// ─── 優先度マッピング ───

fn priority_from_todoist(p: i32) -> Priority {
    match p {
        4 => Priority::High,
        3 => Priority::Medium,
        2 => Priority::Low,
        _ => Priority::None,
    }
}

fn priority_to_todoist(p: Priority) -> i32 {
    match p {
        Priority::High => 4,
        Priority::Medium => 3,
        Priority::Low => 2,
        Priority::None => 1,
    }
}

fn priority_str(p: Priority) -> String {
    match p {
        Priority::High => "high".into(),
        Priority::Medium => "medium".into(),
        Priority::Low => "low".into(),
        Priority::None => "none".into(),
    }
}

fn priority_str_from_todoist(p: i32) -> String {
    priority_str(priority_from_todoist(p))
}

// ─── 日時 ───

fn parse_due(due: Option<&TodoistDue>) -> Option<NaiveDateTime> {
    let due = due?;
    // 旧 REST v2 互換: datetime フィールド
    if let Some(s) = &due.datetime {
        if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
            return Some(dt.with_timezone(&Local).naive_local());
        }
    }
    // v1 unified API: date フィールドは以下の2形式が入りうる
    //   - 日付のみ: "2026-09-15"  → 9:00 にデフォルト
    //   - 時刻付き: "2026-09-15T15:00:00Z" (UTC RFC3339) → ローカル時刻に変換
    if let Some(s) = &due.date {
        if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
            return Some(dt.with_timezone(&Local).naive_local());
        }
        if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            return d.and_hms_opt(9, 0, 0);
        }
    }
    None
}

fn now_rfc3339() -> String {
    Utc.from_utc_datetime(&Utc::now().naive_utc()).to_rfc3339()
}

// ─── 補助 ───

impl Item {
    fn touch(&mut self) {
        self.updated_at = now_rfc3339();
    }
}

fn next_order(locals: &[Item]) -> i64 {
    locals.iter().map(|t| t.manual_order).max().map(|m| m + 1).unwrap_or(0)
}

fn short(s: &str) -> String {
    s.chars().take(24).collect()
}

fn write_atomic(path: &Path, items: &[Item]) -> Result<(), String> {
    let json = serde_json::to_string_pretty(items)
        .map_err(|e| format!("JSON 生成失敗: {e}"))?;
    let tmp = PathBuf::from(TASKS_TMP);
    fs::write(&tmp, &json).map_err(|e| format!("tmp 書込失敗: {e}"))?;
    if path.exists() {
        let _ = fs::copy(path, PathBuf::from(TASKS_BACKUP));
    }
    fs::rename(&tmp, path).map_err(|e| format!("rename 失敗: {e}"))?;
    Ok(())
}
