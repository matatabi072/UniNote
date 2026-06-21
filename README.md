# UniNote

軽量・軽快・環境非依存なローカル **タスク + メモ統合アプリ**。Rust + [egui/eframe](https://github.com/emilk/egui) 製の **単一実行ファイル（約5.3MB）** で、インストール不要・ランタイム不要で動作します。`tasks.json` / `notes.json` は前身 SimpleTask / SimpleNote と互換。外部サービス連携は **サイドカー方式**（独立 exe）でクラッシュ耐性 + 言語自由を実現。

> A lightweight, portable, dependency-free desktop app integrating ToDo and notes, written in Rust (egui). External service sync via independent sidecar executables (loose coupling, language-agnostic).

---

## ✨ 特徴

- **完全ポータブル / インストール不要** — 実行ファイルとデータ（`tasks.json` / `notes.json` / `settings.json`）が同一フォルダで完結。
- **環境非依存** — OpenGL(glow) 描画のネイティブGUI。WebView2 等のランタイム不要。
- **タスク + メモを 1 アプリで** — タブ切替で用途を分離しつつデータ構造は共通。
- **画像メモ対応** — ドラッグ&ドロップで PNG/JPEG/GIF/BMP/WebP を取り込み、サムネ表示（クロップ / ストレッチ選択可）。
- **クラウド同期対応** — 保存フォルダを OneDrive / Dropbox / Google Drive 等に置くだけ。**外部変更を検知して自動再読込**（未保存時は確認ダイアログ）。
- **外部サービス連携** — 別 exe としてサイドカーを `sync/` フォルダに配置するだけで自動検出。GUI ウィザード付き。
- **安定** — JSON破損を検知し、自動バックアップから復元。原子的保存（tmp→rename）。
- **二重起動防止** — 既に起動中なら既存ウィンドウを前面化。

## 🧩 機能

### タスクモード
- 並び替え: 手動（DnD ハンドル）/ 日時順 / 重要度順
- 重要度: 高 / 中 / 低 / なし（右クリックメニュー、背景色カスタマイズ可）
- 予定日時: クリックで日時編集ウィンドウ、表示形式は「日付 / 残り日数」「時刻 / 残り時間」を選択可
- 期限切れ自動赤字表示
- 完了タスクは末尾へ自動移動 + 淡色化

### メモモード
- タイトル（先頭行）一覧 + ホバーで更新日時表示
- ダブルクリックで **別ウィンドウ編集**（位置・サイズ記憶）
- 自動保存（800ms デバウンス）+ 空メモは閉じる時に自動破棄
- 画像メモは「ファイル名のみ / サムネイル（クロップ / ストレッチ）」の3表示モード

### 共通
- テーマ: OSに追従 / ダーク / ライト
- 常に最前面に表示（オン/オフ） — サブウィンドウも自動追従
- フォントファミリー・サイズ即時反映
- 全ウィンドウの位置・サイズを記憶（モニタ範囲外なら自動クランプ）
- 設定ウィンドウはメインウィンドウ相対オフセットで配置記憶

## 🚀 入手・起動

### ビルド済みバイナリ
[Releases](../../releases) から `uninote.exe` をダウンロードし、任意のフォルダに置いてダブルクリック。データは同じフォルダに作成されます。

### ソースからビルド
```bash
cargo build --release   # -> target/release/uninote.exe
cargo test              # データ層の単体テスト
```

## ☁ クラウド同期について

実行ファイルと同階層の `tasks.json` / `notes.json` / `images/` を、クラウド同期フォルダに置いて運用します。別端末で更新された場合、ウィンドウへ戻った時（フォーカス復帰時）に変更を検知し、未保存がなければ自動で再読込、未保存があれば再読込/保持を確認します。

## 🔗 外部サービス連携（サイドカー）

外部サービスとの同期は **独立 exe**（サイドカー）として実装。本体は純粋ローカル動作を保ち、サイドカーが JSON ファイルを読み書きします。

| サイドカー | サービス | 方向 |
|-----------|---------|------|
| [uninote-sync-todoist](https://github.com/matatabi072/uninote-sync-todoist) | Todoist | 双方向 |
| [uninote-sync-gtasks](https://github.com/matatabi072/uninote-sync-gtasks) | Google Tasks | PULL のみ（v1） |

### セットアップ
1. サイドカー exe を `sync/` フォルダに配置
2. UniNote 起動 → **設定（⚙）→ 外部連携 → 🔗 外部連携の設定…** ボタン
3. ウィザードに従って認証情報を入力（トークンは Windows DPAPI で暗号化保存）

### サイドカーを自作する
任意の言語で実装可能です。プロトコル（JSON ファイル契約・終了コード等）は [docs/sidecar-contract.md](docs/sidecar-contract.md) を参照。

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

サイドカーが追加した独自フィールド（例: `todoistId`）も serde の flatten で保持されます。

## 🏗 アーキテクチャ

| ファイル | 役割 |
|----------|------|
| `src/model.rs` | データモデル（タスク/メモ共通 Item） |
| `src/storage.rs` | ローカル保存・破損検知・バックアップ・サイドカー検出 |
| `src/settings.rs` | 設定（フォント・テーマ・最前面・画像表示・ウィンドウ位置） |
| `src/app.rs` | GUI（egui）・タブ切替・サイドカー実行・セットアップウィザード |
| `src/main.rs` | エントリ・二重起動防止・ウィンドウ初期化 |

## 🛠 技術スタック
- 言語: Rust（GNU toolchain）
- GUI: egui / eframe 0.29（glow / OpenGL）
- 画像: egui_extras / image 0.25
- ファイル選択: rfd
- データ: serde_json / 日時: chrono / 識別子: uuid

## 📄 ライセンス
[MIT License](LICENSE)
