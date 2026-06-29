# UniNote

軽量・軽快・環境非依存なローカル **タスク + メモ統合アプリ** とその **外部サービス連携サイドカー群**。すべて Rust 製・Cargo workspace で 1 リポジトリ管理。

- **本体**（約 5.3MB）: タスク管理 + メモ + 画像メモ。`tasks.json` / `notes.json` ローカル保存、クラウド同期フォルダ運用対応。
- **サイドカー**（各 約 1.6MB）: 独立 exe として外部サービス（Todoist / Google Tasks ...）と同期。**本体はクラッシュ耐性のため通信機能を持たない**設計。

> Lightweight, portable, dependency-free desktop app integrating ToDo and notes (Rust + egui), bundled with independent sidecar executables for external service sync (Todoist, Google Tasks). Cargo workspace monorepo.

---

## 📦 構成（Cargo workspace）

```
UniNote/
├ uninote/                          ← 本体 GUI アプリ
│  └ src/
├ sidecars/
│  ├ uninote-sync-todoist/          ← Todoist 双方向同期
│  │  └ src/
│  └ uninote-sync-gtasks/           ← Google Tasks PULL 同期
│     └ src/
├ docs/sidecar-contract.md          ← サイドカー実装契約（公開）
└ Cargo.toml                        ← workspace
```

## ✨ 特徴（本体）

- **完全ポータブル / インストール不要** — 実行ファイルとデータ（`tasks.json` / `notes.json` / `settings.json`）が同一フォルダで完結
- **環境非依存** — OpenGL(glow) 描画のネイティブGUI。WebView2 等のランタイム不要
- **タスク + メモを 1 アプリで** — タブ切替で用途分離・データ構造共通
- **画像メモ対応** — ドラッグ&ドロップで PNG/JPEG/GIF/BMP/WebP を取り込み、サムネ表示（クロップ / ストレッチ）
- **クラウド同期対応** — 保存フォルダを OneDrive / Dropbox / Google Drive 等に置くだけ。**外部変更を検知して自動再読込**（未保存時は確認）
- **外部サービス連携** — サイドカー exe を `sync/` フォルダに配置するだけで GUI 検出。専用ウィザード付き
- **安定** — JSON破損を検知し、自動バックアップから復元。原子的保存（tmp→rename）
- **二重起動防止** — 既に起動中なら既存ウィンドウを前面化

## 🧩 機能

### タスクモード
- 並び替え: 手動（DnD ハンドル）/ 日時順 / 重要度順
- 重要度: 高 / 中 / 低 / なし（右クリックメニュー、背景色カスタマイズ可）
- 予定日時: クリックで日時編集ウィンドウ、表示形式は「日付 / 残り日数」「時刻 / 残り時間」を選択可
- 期限切れ自動赤字表示
- 完了タスクは末尾へ自動移動 + 淡色化
- **タスクをクリックでミニビューワ起動**（別 OS ウィンドウ）— 本文閲覧・編集、URL を青リンクで表示してクリックで既定ブラウザ起動、「同じタスク作成」で複製を編集モードで開く
- **ホバープレビュー** — タスクにポインターを乗せると、メインウィンドウの右側（収まらなければ左側）に本文プレビュー（別 OS ウィンドウ・クリックスルー）を表示

### メモモード
- タイトル（先頭行）一覧 + ホバーで更新日時表示
- ダブルクリックで **別ウィンドウ編集**（位置・サイズ記憶）
- 自動保存（800ms デバウンス）+ 空メモは閉じる時に自動破棄
- 画像メモは「ファイル名のみ / サムネイル（クロップ / ストレッチ）」の 3 表示モード
- DnD ハンドルで手動並び替え

### 共通
- テーマ: OSに追従 / ダーク / ライト
- 常に最前面に表示 — サブウィンドウも自動追従
- フォントファミリー・サイズ即時反映
- 全ウィンドウの位置・サイズを記憶（モニタ範囲外なら自動クランプ）
- 設定ウィンドウはメインウィンドウ相対オフセットで配置記憶
- スクロールバー常時表示（位置インジケータが常に視認可能）

### 自動同期（v0.2.0〜）
- **起動時に同期**（ON / OFF 切替可）+ **N 分間隔の定期同期**（既定 15 分、0 で無効）
- サイドカー検出が必要。複数サイドカー（Todoist + Google Tasks 等）は順次実行（API レート対策・JSON 書込競合回避）
- サイレント実行: 失敗時のみ上部バナーに警告。手動同期中・ビューワ編集中は一時停止して未保存編集の喪失を防止

## 🚀 入手・起動

### ビルド済みバイナリ
[Releases](../../releases) から必要なバイナリをダウンロード：

- `uninote.exe` （本体・必須）
- `uninote-sync-todoist.exe` （任意）
- `uninote-sync-gtasks.exe` （任意）

`uninote.exe` を任意のフォルダに置いてダブルクリック。サイドカーを使うなら同フォルダの `sync/` 配下に置きます。

### ソースからビルド
```bash
cargo build --release   # 全 3 バイナリ → target/release/
```

個別ビルド:
```bash
cargo build --release -p uninote
cargo build --release -p uninote-sync-todoist
cargo build --release -p uninote-sync-gtasks
```

## ☁ クラウド同期について

実行ファイルと同階層の `tasks.json` / `notes.json` / `images/` を、クラウド同期フォルダに置いて運用します。別端末で更新された場合、ウィンドウへ戻った時（フォーカス復帰時）に変更を検知し、未保存がなければ自動で再読込、未保存があれば再読込/保持を確認します。

## 🔗 外部サービス連携

### 同梱サイドカー

| サイドカー | サービス | 方向 | 認証 | 詳細 |
|-----------|---------|------|------|------|
| [uninote-sync-todoist](sidecars/uninote-sync-todoist/) | Todoist | 双方向 | API トークン | 3-way merge / 競合 remote-wins / 削除 push |
| [uninote-sync-gtasks](sidecars/uninote-sync-gtasks/) | Google Tasks | 双方向（v2） | OAuth 2.0 | 3-way merge / 競合 remote-wins / 削除 push / refresh 自動 / tombstones |

### セットアップ手順（共通）
1. サイドカー exe を `sync/` フォルダに配置
2. UniNote 起動 → **設定（⚙）→ 外部連携 → 🔗 外部連携の設定…** ボタン
3. ウィザードに従って認証情報を入力（トークンは Windows DPAPI で暗号化保存）
4. メイン画面の「🔄 同期」ボタンから実行

### サイドカーを自作する
任意の言語で実装可能です。プロトコル（JSON ファイル契約・配置ルール・終了コード等）は [docs/sidecar-contract.md](docs/sidecar-contract.md) を参照。

## 🔌 関連ツール（プラグイン）

外部 API 連携とは別に、**ローカル機能を拡張する独立 GUI/CLI ツール** を `tools/` フォルダに配置すると自動検出されます。本体の設定 →「関連ツール」セクションから起動可能。各ツールは UniNote なしでも単独動作するよう設計されています。

| ツール | 機能 |
|--------|------|
| **SimpleCalendar** | tasks.json と連携するカレンダー表示（月/週/日）。`--linked-tasks` で UniNote の tasks.json を read-merge-write 共同管理 |
| **PCReminder** | scheduledDateTime に応じて PC スリープ/復帰/通知を Windows タスクスケジューラに予約。CLI ベースで本体クラッシュ耐性あり |

配置例:
```
UniNote/
├ uninote.exe
├ tasks.json
├ tools/
│  ├ simplecalendar.exe
│  ├ pc-reminder.exe
│  └ pc-reminder-gui.exe
└ sync/  ← サイドカーは別フォルダ
```

## 🗂 データ構造

`tasks.json` と `notes.json` は同一スキーマ：

```json
{
  "id": "uuid",
  "googleTaskId": null,
  "taskContent": "内容",
  "isCompleted": false,
  "scheduledDateTime": null,
  "priority": "none",
  "manualOrder": 0,
  "updatedAt": "2026-06-21T00:00:00Z",
  "kind": "text",
  "imageName": null
}
```

サイドカーが追加した独自フィールド（例: `todoistId`）も serde の flatten で本体側も保持。

## 🏗 アーキテクチャ

### 本体（`uninote/src/`）
| ファイル | 役割 |
|----------|------|
| `model.rs` | データモデル（タスク/メモ共通 Item） |
| `storage.rs` | ローカル保存・破損検知・バックアップ・サイドカー検出 |
| `settings.rs` | 設定（フォント・テーマ・最前面・画像表示・ウィンドウ位置） |
| `app.rs` | GUI（egui）・タブ切替・サイドカー実行・セットアップウィザード |
| `main.rs` | エントリ・二重起動防止・ウィンドウ初期化 |

### サイドカーランタイム
本体は `Command::new()` でサイドカーを子プロセスとして起動し、stdout/stderr を `mpsc` 経由で GUI ログに流します。`CREATE_NO_WINDOW` でターミナル非表示。完了後は `tasks.json` の mtime 変化を検出し自動再読込（未保存編集ありなら競合ダイアログ）。

## 🛠 技術スタック

| 用途 | クレート |
|------|---------|
| GUI | eframe / egui 0.29（glow） |
| 画像 | egui_extras / image 0.25 |
| ファイル選択 | rfd |
| データ | serde / serde_json |
| 日時 | chrono |
| 識別子 | uuid |
| HTTP（サイドカー）| ureq |
| 認証情報暗号化（Windows）| windows-sys（DPAPI） |

## 📄 ライセンス
[MIT License](LICENSE) — Copyright (c) 2026 matatabi072
