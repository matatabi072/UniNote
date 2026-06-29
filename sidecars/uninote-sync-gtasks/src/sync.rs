//! Google Tasks ↔ UniNote 双方向同期（v2）。
//!
//! 3-way merge: 前回同期スナップショット（state.json::synced_tasks）を基準に
//! ローカル変更 / リモート変更 / 両方変更 を判別する。
//!
//! ポリシー（Todoist v2 と同じ）:
//! - 競合（両方変更）: REMOTE WINS（Google を正本扱い、ローカル変更は警告ログ）
//! - ローカルから消えたタスクの remote 側削除: する（push DELETE）
//! - 初回 v2 移行（synced_tasks なし + 既存 google_task_id あり）:
//!     現状をベースラインに採用して誤上書き防止
//!
//! Google Tasks 固有制約:
//! - 優先度概念なし → UniNote の priority は無視（push しない / pull で None 固定）
//! - due は日付のみ（時刻部分は無視される）
use crate::gtasks::{self, GTask, TaskMutation};
use crate::model::{Item, Priority};
use crate::state::{self, SyncState, TaskSnapshot};
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use std::collections::{BTreeMap, BTreeSet};
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
    pub skipped_tombstone: usize,
    pub errors: usize,
}

pub fn run(
    client: &mut gtasks::Client,
    remote: &[GTask],
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

    // 削除/非表示タスクは無視
    let usable: Vec<&GTask> = remote.iter().filter(|t| !t.deleted && !t.hidden).collect();

    // 初回 v2 移行: synced_tasks が空で既存の google_task_id 付きローカルがある
    // → 現状をベースラインに採用（ローカル変更を誤って上書きしない）
    if state.synced_tasks.is_empty()
        && locals.iter().any(|l| l.google_task_id.is_some())
    {
        for l in locals.iter() {
            if let Some(gid) = &l.google_task_id {
                state
                    .synced_tasks
                    .insert(gid.clone(), snapshot_of_local(l));
            }
        }
        println!(
            "[info] 初回 v2 同期: ローカル {} 件をベースラインに記録",
            state.synced_tasks.len()
        );
    }

    let mut report = SyncReport {
        fetched: usable.len(),
        ..Default::default()
    };
    let mut tasks_changed = false;
    let mut new_state = SyncState {
        version: 2,
        last_sync_at: Some(now_rfc3339()),
        last_known_ids: BTreeSet::new(),
        tombstones: state.tombstones.clone(),
        synced_tasks: BTreeMap::new(),
    };

    let remote_by_id: BTreeMap<String, &GTask> =
        usable.iter().map(|t| (t.id.clone(), *t)).collect();

    // ── Phase 0: ローカル削除を検出 → Google 側にも削除を push ──
    // state.synced_tasks にあるが、現在のローカルから消えている = ユーザーが削除した
    let local_gids: BTreeSet<String> =
        locals.iter().filter_map(|t| t.google_task_id.clone()).collect();
    let deleted_locally: Vec<String> = state
        .synced_tasks
        .keys()
        .filter(|gid| !local_gids.contains(*gid))
        .cloned()
        .collect();
    let mut just_deleted: BTreeSet<String> = BTreeSet::new();
    for gid in &deleted_locally {
        if dry_run {
            println!("[dry] push 削除: google_task_id={gid}");
            just_deleted.insert(gid.clone());
            // 同時に tombstone も追加（次回 sync で復活させない）
            new_state.tombstones.insert(gid.clone());
            continue;
        }
        match client.delete_task(gid) {
            Ok(()) => {
                println!("[sync] push 削除: google_task_id={gid}");
                report.pushed_deletes += 1;
                just_deleted.insert(gid.clone());
                new_state.tombstones.insert(gid.clone());
            }
            Err(e) => {
                eprintln!("push 削除失敗 [{gid}]: {e}");
                report.errors += 1;
                // 失敗時は state を維持して次回再試行
                if let Some(s) = state.synced_tasks.get(gid).cloned() {
                    new_state.synced_tasks.insert(gid.clone(), s);
                }
            }
        }
    }

    // ── Phase A: 既存ペア（google_task_id を持つローカル）を 3-way merge ──
    for local in locals.iter_mut() {
        let Some(gid) = local.google_task_id.clone() else {
            continue;
        };
        let prev = state.synced_tasks.get(&gid).cloned();
        match remote_by_id.get(gid.as_str()) {
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
                                eprintln!(
                                    "push 更新失敗 [{}]: {e}",
                                    short(&local.task_content)
                                );
                                report.errors += 1;
                            }
                        }
                    }
                } else {
                    report.unchanged += 1;
                }
                new_state
                    .synced_tasks
                    .insert(gid, snapshot_of_local(local));
            }
            None => {
                // リモートから消えた（削除/非表示）→ ローカルは維持しつつ
                // state.synced_tasks も維持（次回の Phase 0 判定で誤って push 削除しないため）
                if let Some(s) = prev {
                    new_state.synced_tasks.insert(gid, s);
                }
            }
        }
    }

    // ── Phase B: Google にあって local に無い → 追加（tombstone は除外） ──
    for r in &usable {
        if local_gids.contains(&r.id) || just_deleted.contains(&r.id) {
            continue;
        }
        if new_state.tombstones.contains(&r.id) {
            report.skipped_tombstone += 1;
            continue;
        }
        // ローカルに既存（同 id）はこの時点で local_gids 由来で除外済みなので、
        // 純粋に「Google 側のみに存在」する新規タスク
        let item = item_from_gtask(r, next_order(&locals));
        new_state
            .synced_tasks
            .insert(r.id.clone(), snapshot_of_local(&item));
        locals.push(item);
        report.added_from_remote += 1;
        tasks_changed = true;
    }

    // ── Phase C: 純ローカル（google_task_id なし）→ Google に CREATE ──
    let to_create_indices: Vec<usize> = locals
        .iter()
        .enumerate()
        .filter(|(_, l)| l.google_task_id.is_none())
        .map(|(i, _)| i)
        .collect();
    for i in to_create_indices {
        if dry_run {
            println!("[dry] push 新規: {}", short(&locals[i].task_content));
            continue;
        }
        let body = build_mutation_create(&locals[i]);
        match client.create_task(&body) {
            Ok(created) => {
                println!(
                    "[sync] push 新規: {} → google_task_id={}",
                    short(&locals[i].task_content),
                    created.id
                );
                locals[i].google_task_id = Some(created.id.clone());
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

    // last_known_ids 更新（次回起動時の削除検出用）
    new_state.last_known_ids = locals
        .iter()
        .filter_map(|it| it.google_task_id.clone())
        .collect();

    // ── 状態とローカルを書き戻し ──
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
    local.task_content != snap.title
        || local.is_completed != snap.is_completed
        || local.scheduled_date_time != snap.scheduled_date_time
}

fn differs_remote(remote: &GTask, snap: &TaskSnapshot) -> bool {
    remote.title != snap.title
        || (remote.status == "completed") != snap.is_completed
        || parse_due(remote.due.as_deref()) != snap.scheduled_date_time
}

// ─── 適用 ───

fn apply_remote(local: &mut Item, r: &GTask) {
    local.task_content = r.title.clone();
    local.is_completed = r.status == "completed";
    local.scheduled_date_time = parse_due(r.due.as_deref());
    if local.google_task_id.as_deref() != Some(r.id.as_str()) {
        local.google_task_id = Some(r.id.clone());
    }
    local.touch();
}

/// ローカルの変更を Google に push（完了状態の差も同じ PATCH リクエストで送る）。
/// 戻り値: 完了状態が変更されたか
fn push_local_to_remote(
    client: &mut gtasks::Client,
    remote: &GTask,
    local: &Item,
) -> Result<bool, String> {
    let completion_changed = local.is_completed != (remote.status == "completed");
    let body = build_mutation_update(local);
    client.update_task(&remote.id, &body)?;
    Ok(completion_changed)
}

// ─── 変換 ───

fn build_mutation_create(local: &Item) -> TaskMutation {
    TaskMutation {
        title: if local.task_content.trim().is_empty() {
            "(無題)".to_string()
        } else {
            local.task_content.clone()
        },
        status: Some(if local.is_completed {
            "completed".into()
        } else {
            "needsAction".into()
        }),
        due: date_to_gtasks_due(local.scheduled_date_time),
    }
}

fn build_mutation_update(local: &Item) -> TaskMutation {
    TaskMutation {
        title: if local.task_content.trim().is_empty() {
            "(無題)".to_string()
        } else {
            local.task_content.clone()
        },
        status: Some(if local.is_completed {
            "completed".into()
        } else {
            "needsAction".into()
        }),
        due: date_to_gtasks_due(local.scheduled_date_time),
    }
}

/// ローカル NaiveDateTime → Google Tasks の due 文字列。
/// Google Tasks は日付のみ保存するため、時刻部分は捨てて UTC 00:00:00 で送る。
fn date_to_gtasks_due(dt: Option<NaiveDateTime>) -> Option<String> {
    let dt = dt?;
    let date = dt.date();
    Some(format!(
        "{}T00:00:00.000Z",
        date.format("%Y-%m-%d")
    ))
}

fn item_from_gtask(r: &GTask, order: i64) -> Item {
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

fn snapshot_of_local(l: &Item) -> TaskSnapshot {
    TaskSnapshot {
        title: l.task_content.clone(),
        is_completed: l.is_completed,
        scheduled_date_time: l.scheduled_date_time,
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
