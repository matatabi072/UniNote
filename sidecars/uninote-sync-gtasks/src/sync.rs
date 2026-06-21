//! Google Tasks → UniNote 一方向 PULL 同期（v1）。
//!
//! 注意: `googleTaskId` フィールドは Todoist サイドカーでは「外部 ID 共通格納用」と
//! 解釈していたが、本サイドカーでは Google Tasks の ID 専用にする。
//! Todoist サイドカーは todoist_id を別フィールドで管理しているので衝突しない。
//!
//! ユーザーがローカルで削除したタスクが remote 取得で復活しないよう、
//! state.json の tombstones で抑止する。
use crate::gtasks::GTask;
use crate::model::{Item, Priority};
use crate::state::{self, SyncState};
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const TASKS_FILE: &str = "tasks.json";
const TASKS_BACKUP: &str = "tasks.backup.json";
const TASKS_TMP: &str = "tasks.json.tmp";

pub struct SyncReport {
    pub fetched: usize,
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub skipped_tombstone: usize,
    pub new_tombstones: usize,
}

pub fn run(remote: &[GTask]) -> Result<SyncReport, String> {
    let tasks_path = PathBuf::from(TASKS_FILE);
    let mut locals: Vec<Item> = if tasks_path.exists() {
        let raw = fs::read_to_string(&tasks_path)
            .map_err(|e| format!("{TASKS_FILE} 読込失敗: {e}"))?;
        serde_json::from_str(&raw).map_err(|e| format!("{TASKS_FILE} 解析失敗: {e}"))?
    } else {
        Vec::new()
    };

    let mut state = state::load();

    // 削除/非表示タスクは無視
    let usable: Vec<&GTask> = remote.iter().filter(|t| !t.deleted && !t.hidden).collect();

    // ── 削除検出: 前回 sync 時のローカル ID 集合 - 現在のローカル ID 集合 = 削除されたもの ──
    let current_local_ids: BTreeSet<String> = locals
        .iter()
        .filter_map(|it| it.google_task_id.clone())
        .collect();
    let newly_deleted: BTreeSet<String> = state
        .last_known_ids
        .difference(&current_local_ids)
        .cloned()
        .collect();
    let new_tombstones_count = newly_deleted.len();
    state.tombstones.extend(newly_deleted);

    let mut added = 0usize;
    let mut updated = 0usize;
    let mut unchanged = 0usize;
    let mut skipped_tombstone = 0usize;

    for r in &usable {
        // tombstone にあるものは新規追加しない（ローカルに既存ならそちらが優先）
        let in_tombstone = state.tombstones.contains(&r.id);

        let idx = locals
            .iter()
            .position(|it| it.google_task_id.as_deref() == Some(r.id.as_str()));
        if let Some(i) = idx {
            // ローカルに存在する → tombstone から復活（ユーザーが手動で再作成したかも）
            state.tombstones.remove(&r.id);
            if apply_update(&mut locals[i], r) {
                locals[i].updated_at = now_rfc3339();
                updated += 1;
            } else {
                unchanged += 1;
            }
        } else if in_tombstone {
            // 削除済みなので再追加しない
            skipped_tombstone += 1;
        } else {
            let next_order = locals
                .iter()
                .map(|it| it.manual_order)
                .max()
                .map(|m| m + 1)
                .unwrap_or(0);
            locals.push(from_gtask(r, next_order));
            added += 1;
        }
    }

    if added > 0 || updated > 0 {
        write_atomic(&tasks_path, &locals)?;
    }

    // 状態保存（毎回更新、ローカル ID スナップショット）
    let new_state = SyncState {
        version: 1,
        last_sync_at: Some(now_rfc3339()),
        last_known_ids: locals
            .iter()
            .filter_map(|it| it.google_task_id.clone())
            .collect(),
        tombstones: state.tombstones.clone(),
    };
    if let Err(e) = state::save(&new_state) {
        eprintln!("state 保存失敗: {e}");
    }

    Ok(SyncReport {
        fetched: usable.len(),
        added,
        updated,
        unchanged,
        skipped_tombstone,
        new_tombstones: new_tombstones_count,
    })
}

fn apply_update(item: &mut Item, r: &GTask) -> bool {
    let mut changed = false;
    if item.task_content != r.title {
        item.task_content = r.title.clone();
        changed = true;
    }
    let completed = r.status == "completed";
    if item.is_completed != completed {
        item.is_completed = completed;
        changed = true;
    }
    let new_dt = parse_due(r.due.as_deref());
    if item.scheduled_date_time != new_dt {
        item.scheduled_date_time = new_dt;
        changed = true;
    }
    if item.google_task_id.as_deref() != Some(r.id.as_str()) {
        item.google_task_id = Some(r.id.clone());
        changed = true;
    }
    changed
}

fn from_gtask(r: &GTask, order: i64) -> Item {
    Item {
        id: uuid::Uuid::new_v4().to_string(),
        google_task_id: Some(r.id.clone()),
        task_content: r.title.clone(),
        is_completed: r.status == "completed",
        scheduled_date_time: parse_due(r.due.as_deref()),
        priority: Priority::None, // Google Tasks に優先度概念なし
        manual_order: order,
        updated_at: now_rfc3339(),
        kind: "text".to_string(),
        image_name: None,
        todoist_id: None,
        extra: Default::default(),
    }
}

/// Google Tasks の due は常に UTC 0時の datetime（"2026-09-15T00:00:00.000Z"）。
/// 時刻部分は意味を持たないため、ローカル時刻 9:00 に正規化する。
fn parse_due(due: Option<&str>) -> Option<NaiveDateTime> {
    let s = due?;
    let dt = DateTime::parse_from_rfc3339(s).ok()?;
    let date = dt.with_timezone(&Utc).date_naive();
    date.and_hms_opt(9, 0, 0)
}

fn now_rfc3339() -> String {
    Utc.from_utc_datetime(&Utc::now().naive_utc()).to_rfc3339()
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
