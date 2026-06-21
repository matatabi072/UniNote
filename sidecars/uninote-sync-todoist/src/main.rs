//! uninote-sync-todoist
//! UniNote サイドカー: Todoist REST v2 から tasks.json へ一方向 PULL 同期。
//!
//! Usage:
//!   uninote-sync-todoist                 # 同期実行（本体から呼び出される標準形式）
//!   uninote-sync-todoist --set-token T   # トークンを DPAPI 暗号化して保存
//!   uninote-sync-todoist --clear-token   # 保存済みトークンを削除
//!   uninote-sync-todoist --status        # トークン保存状況を表示
//!   uninote-sync-todoist --help          # ヘルプ
//!
//! 終了コード:
//!   0 成功 / 2 設定不足 / 3 ネットワーク / 4 認証失敗 / 1 その他失敗

mod dpapi;
mod model;
mod state;
mod sync;
mod todoist;
mod token_store;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(|s| s.as_str()) {
        Some("--help") | Some("-h") => {
            print_help();
            ExitCode::from(0)
        }
        Some("--set-token") => match args.get(1) {
            Some(t) => set_token(t),
            None => {
                eprintln!("--set-token にはトークンを指定してください");
                ExitCode::from(64)
            }
        },
        Some("--clear-token") => clear_token(),
        Some("--status") => status(),
        Some("--dry-run") => run_sync(true),
        None => run_sync(false),
        Some(other) => {
            eprintln!("不明な引数: {other}");
            print_help();
            ExitCode::from(64)
        }
    }
}

fn print_help() {
    println!("uninote-sync-todoist v{}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("UniNote サイドカー: Todoist ↔ tasks.json 双方向同期 (v2)");
    println!();
    println!("Usage:");
    println!("  uninote-sync-todoist                 同期実行");
    println!("  uninote-sync-todoist --dry-run       実通信せず計画だけ表示");
    println!("  uninote-sync-todoist --set-token T   トークンを DPAPI 暗号化保存");
    println!("  uninote-sync-todoist --clear-token   保存済みトークン削除");
    println!("  uninote-sync-todoist --status        状態表示");
    println!("  uninote-sync-todoist --help          このヘルプ");
}

fn set_token(token: &str) -> ExitCode {
    let t = token.trim();
    if t.is_empty() {
        eprintln!("空のトークンは保存できません");
        return ExitCode::from(64);
    }
    match token_store::save(t) {
        Ok(()) => {
            println!("[info] トークンを DPAPI 暗号化して保存しました");
            ExitCode::from(0)
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}

fn clear_token() -> ExitCode {
    match token_store::clear() {
        Ok(true) => {
            println!("[info] 保存済みトークンを削除しました");
            ExitCode::from(0)
        }
        Ok(false) => {
            println!("[info] 保存済みトークンはありませんでした");
            ExitCode::from(0)
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}

fn status() -> ExitCode {
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "(不明)".into());
    println!("[info] 作業ディレクトリ: {cwd}");
    if token_store::exists() {
        println!("[info] トークン: 設定済み");
    } else {
        println!("[info] トークン: 未設定");
    }
    ExitCode::from(0)
}

fn run_sync(dry_run: bool) -> ExitCode {
    let token = match token_store::load() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("トークン取得失敗: {e}");
            eprintln!("初回設定: uninote-sync-todoist --set-token <YOUR_API_TOKEN>");
            return ExitCode::from(2);
        }
    };

    if dry_run {
        println!("[info] DRY-RUN モード（変更は書き込まれません）");
    }
    println!("[info] Todoist 接続中…");
    let client = todoist::Client::new(token);
    let tasks = match client.list_active_tasks() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return if e.contains("認証") {
                ExitCode::from(4)
            } else if e.contains("通信") {
                ExitCode::from(3)
            } else {
                ExitCode::from(1)
            };
        }
    };
    println!("[sync] 取得: {}件", tasks.len());

    match sync::run(&client, &tasks, dry_run) {
        Ok(r) => {
            println!(
                "[sync] PULL: 追加={} 更新={} (取得={})",
                r.added_from_remote, r.pulled_updates, r.fetched
            );
            println!(
                "[sync] PUSH: 新規={} 更新={} 完了状態={} 削除={}",
                r.pushed_creates, r.pushed_updates, r.completion_pushed, r.pushed_deletes
            );
            if r.conflicts_remote_wins > 0 {
                println!(
                    "[sync] 競合(remote勝ち): {} 件",
                    r.conflicts_remote_wins
                );
            }
            println!(
                "[sync] 変更なし: {} 件 / エラー: {} 件",
                r.unchanged, r.errors
            );
            if dry_run {
                println!("[info] DRY-RUN 終了（何も書き込まれていません）");
            } else {
                println!("[info] 完了");
            }
            if r.errors > 0 {
                ExitCode::from(1)
            } else {
                ExitCode::from(0)
            }
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}
