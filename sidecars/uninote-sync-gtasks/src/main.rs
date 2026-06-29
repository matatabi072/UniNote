//! uninote-sync-gtasks v0.2
//! UniNote サイドカー: Google Tasks ↔ tasks.json 双方向同期。
//!
//! セットアップ手順:
//!   1) Google Cloud Console で Tasks API 有効化 + OAuth Desktop client 作成
//!   2) uninote-sync-gtasks --setup <CLIENT_ID> <CLIENT_SECRET>
//!   3) uninote-sync-gtasks --auth    （ブラウザで認可）
//!   4) uninote-sync-gtasks           （以後は引数なしで同期実行）
//!
//! 双方向ポリシー:
//!   - 競合（両方変更）: REMOTE WINS（Google 側を正本扱い、ローカル変更は破棄+警告）
//!   - ローカル削除: Google からも削除 + tombstone に追加（次回の復活を抑止）
//!   - 純ローカル新規: Google に CREATE → 取得した id を googleTaskId に保存
//!
//! Google Tasks 固有制約:
//!   - 優先度の概念なし（UniNote の priority は push しない / pull で None）
//!   - due は日付のみ（時刻部分は無視）
//!
//! 終了コード:
//!   0 成功 / 1 一般失敗 / 2 設定不足 / 3 通信失敗 / 4 認証失敗 / 64 引数誤り

mod creds_store;
mod dpapi;
mod gtasks;
mod model;
mod oauth;
mod state;
mod sync;
mod token_store;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let positional: Vec<&str> =
        args.iter().filter(|a| a.as_str() != "--dry-run").map(|s| s.as_str()).collect();
    match positional.first().copied() {
        Some("--help") | Some("-h") => {
            print_help();
            ExitCode::from(0)
        }
        Some("--setup") => match (positional.get(1), positional.get(2)) {
            (Some(cid), Some(secret)) => setup(cid, secret),
            _ => {
                eprintln!("--setup <CLIENT_ID> <CLIENT_SECRET> の形式で指定してください");
                ExitCode::from(64)
            }
        },
        Some("--auth") => auth(),
        Some("--clear-auth") => clear_tokens(),
        Some("--clear-all") => clear_all(),
        Some("--status") => status(),
        None => run_sync(dry_run),
        Some(other) => {
            eprintln!("不明な引数: {other}");
            print_help();
            ExitCode::from(64)
        }
    }
}

fn print_help() {
    println!("uninote-sync-gtasks v{}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("UniNote サイドカー: Google Tasks ↔ tasks.json 双方向同期");
    println!();
    println!("Usage:");
    println!("  uninote-sync-gtasks                       同期実行（双方向）");
    println!("  uninote-sync-gtasks --dry-run             書き込みなし試走（差分のみ表示）");
    println!("  uninote-sync-gtasks --setup CID SECRET    OAuth クレデンシャル保存");
    println!("  uninote-sync-gtasks --auth                ブラウザで認可フロー実行");
    println!("  uninote-sync-gtasks --clear-auth          トークン削除（クレデンシャルは残る）");
    println!("  uninote-sync-gtasks --clear-all           トークン+クレデンシャル削除");
    println!("  uninote-sync-gtasks --status              状態表示");
    println!("  uninote-sync-gtasks --help                このヘルプ");
    println!();
    println!("環境変数:");
    println!("  UNINOTE_SYNC_DEBUG=1                       API リクエスト/レスポンスをログ");
    println!();
    println!("初回セットアップ:");
    println!("  1) https://console.cloud.google.com/ で Tasks API 有効化");
    println!("  2) OAuth client (Desktop) を作成し、CLIENT_ID と CLIENT_SECRET を取得");
    println!("  3) --setup <CLIENT_ID> <CLIENT_SECRET>");
    println!("  4) --auth （ブラウザで認可）");
}

fn setup(client_id: &str, client_secret: &str) -> ExitCode {
    let creds = creds_store::ClientCredentials {
        client_id: client_id.trim().to_string(),
        client_secret: client_secret.trim().to_string(),
    };
    if creds.client_id.is_empty() || creds.client_secret.is_empty() {
        eprintln!("空の値は保存できません");
        return ExitCode::from(64);
    }
    match creds_store::save(&creds) {
        Ok(()) => {
            println!("[info] クレデンシャルを DPAPI 暗号化して保存しました");
            println!("[info] 次に --auth を実行してください");
            ExitCode::from(0)
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}

fn auth() -> ExitCode {
    let creds = match creds_store::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("クレデンシャル取得失敗: {e}");
            eprintln!("初回設定: --setup <CLIENT_ID> <CLIENT_SECRET>");
            return ExitCode::from(2);
        }
    };
    let tokens = match oauth::run_authorization(&creds) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("認可失敗: {e}");
            return ExitCode::from(4);
        }
    };
    if let Err(e) = token_store::save(&tokens) {
        eprintln!("{e}");
        return ExitCode::from(1);
    }
    println!("[info] 認可成功。トークンを DPAPI 暗号化保存しました");
    println!("[info] 以後は引数なしで同期実行できます");
    ExitCode::from(0)
}

fn clear_tokens() -> ExitCode {
    match token_store::clear() {
        Ok(true) => {
            println!("[info] トークンを削除しました（クレデンシャルは残ります）");
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

fn clear_all() -> ExitCode {
    let _ = token_store::clear();
    let _ = creds_store::clear();
    println!("[info] トークン+クレデンシャルを削除しました");
    ExitCode::from(0)
}

fn status() -> ExitCode {
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "(不明)".into());
    println!("[info] 作業ディレクトリ: {cwd}");
    if creds_store::exists() {
        println!("[info] クレデンシャル: 設定済み");
    } else {
        println!("[info] クレデンシャル: 未設定");
    }
    if token_store::exists() {
        println!("[info] トークン: 設定済み");
    } else {
        println!("[info] トークン: 未設定");
    }
    ExitCode::from(0)
}

fn run_sync(dry_run: bool) -> ExitCode {
    let creds = match creds_store::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("クレデンシャル取得失敗: {e}");
            eprintln!("初回設定: --setup <CLIENT_ID> <CLIENT_SECRET>");
            return ExitCode::from(2);
        }
    };
    let tokens = match token_store::load() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("トークン取得失敗: {e}");
            eprintln!("認可: --auth");
            return ExitCode::from(2);
        }
    };

    if dry_run {
        println!("[info] DRY RUN モード: 書き込みなし、差分のみ表示");
    }
    println!("[info] Google Tasks 接続中…");
    let mut client = gtasks::Client::new(creds, tokens);
    let tasks = match client.list_all_tasks() {
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
    println!("[sync] 取得: {}件（削除/非表示除外前）", tasks.len());

    match sync::run(&mut client, &tasks, dry_run) {
        Ok(r) => {
            println!("[sync] === 反映結果 (有効件数={}) ===", r.fetched);
            println!(
                "[sync] 取込(remote→local): 追加={} 更新={}",
                r.added_from_remote, r.pulled_updates
            );
            println!(
                "[sync] 送信(local→remote): 新規={} 更新={} 削除={}",
                r.pushed_creates, r.pushed_updates, r.pushed_deletes
            );
            if r.conflicts_remote_wins > 0 {
                println!(
                    "[warn] 競合(remote勝ち)={} 件 / 完了状態push={} 件",
                    r.conflicts_remote_wins, r.completion_pushed
                );
            }
            println!(
                "[sync] 変更なし={} / 復活抑止={} / エラー={}",
                r.unchanged, r.skipped_tombstone, r.errors
            );
            println!("[info] 完了");
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
