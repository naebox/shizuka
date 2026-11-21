<p align="center">
  <img src="./assets/shizuka-icon.svg" alt="Shizuka" width="96" height="96" />
</p>

<h1 align="center">Shizuka</h1>

<p align="center">
  ローカル完結・リアルタイム AI マイクノイズキャンセリングアプリ (Windows / macOS)<br />
  Powered by <strong>DeepFilterNet3</strong> + <strong>Neural VAD Gate</strong> · Rust / Tauri v2 · MIT License
</p>

---

**Shizuka（静か）** は、クラウド送信なしで動作するタスクトレイ常駐型のリアルタイム AI マイクノイズ除去アプリです。キーボード打鍵音・エアコン音・周囲の生活音を強力に抑えつつ、自然な声だけを仮想マイク経由で Discord / Zoom / Teams / Google Meet 等へ届けます。プロジェクト全体が **MIT ライセンス**（他社プロプライエタリ／GPL バイナリの同梱なし）で構成されています。

## ダウンロード（ビルド済みアプリ）

最新版のインストーラ／実行ファイルは **[GitHub Releases](https://github.com/seedling-nae/shizuka/releases/latest)** からそのままダウンロードできます。

| OS | ファイル | 説明 |
| --- | --- | --- |
| **Windows (x64)** | `Shizuka_*_x64-setup.exe` | 推奨インストーラ（NSIS・日本語対応・自動更新対応） |
| **Windows (x64)** | `Shizuka_*_windows_x64_portable.zip` | インストール不要のポータブル版（更新通知→Releases 案内） |
| **Windows (x64)** | `Shizuka_*_x64_en-US.msi` | MSI パッケージ（自動更新対応） |
| **macOS (Apple Silicon)** | `Shizuka_*_aarch64.dmg` | M1 以降搭載 Mac 向け（自動更新対応） |
| **macOS (Intel)** | `Shizuka_*_x64.dmg` | Intel Mac 向け（自動更新対応） |

> [!TIP]
> 各リリースには全配布ファイルのハッシュ値をまとめた `SHA256SUMS.txt` が自動添付されます。また、`v0.4.0` 以降は Ed25519 署名検証付きのアプリ内ワンクリック自動アップデートに対応しています。

> [!NOTE]
> 個人ビルド（未署名）のため、初回起動時に OS の警告が表示される場合があります。
> - **Windows**: SmartScreen が表示された場合は「詳細情報」→「実行」をクリックしてください。
> - **macOS**: 初回はアプリを右クリック →「開く」（または「システム設定 → プライバシーとセキュリティ → このまま開く」）から起動してください。

## 主な特徴

- **2 つの DeepFilterNet3 モデルをアプリ内で即時切替**
  - **高精度 `DeepFilterNet3`（既定・20ms 先読み）**: 未来 2 フレームを参照して突発的な打鍵音や環境音を高精度に分離
  - **超低遅延 `DeepFilterNet3_ll`（0ms 先読み）**: ライブモニター用途など極限の低遅延が欲しいときにワンクリックで切替
- **ニューラル `lsnr` 連動・無音時ノイズゲート（VAD）＆音質コンディショニング（HPF / AGC）**
  - DeepFilterNet3 がフレームごとに推定する局所 SNR（`lsnr`）＋ 3.5 dB ヒステリシス＋ 180ms ホールド＋サンプル単位の滑らかなエンベロープにより、語尾や話し始めを切らずに非発話時の残留ノイズを完全ミュート
  - AI 推論前の **65 Hz Butterworth ハイパスフィルタ（HPF）** で机振動・吹かれ音をカットし、発話区間のみで追従する **VAD 連動オートゲイン（AGC）**＋リミッター作動時の **`CLIP` 警告**、さらに物理マイク名ごとの設定自動記憶に対応
- **グローバルショートカット＆一括マイクミュート**
  - 「ノイズ除去 ON/OFF」と「マイクミュート」を任意のグローバルショートカットに登録可能（初期値未割当）。ミュート専用の赤 LED トレイアイコンと控えめな合成操作音（設定で消音可）でフルスクリーンゲームや会議中もハンズフリー操作が可能
- **長時間通話でも音割れしないクロックドリフト補正**
  - 物理マイクと仮想オーディオデバイス間のクロック誤差を PI 制御＋非同期リサンプラー（`rubato`）で常時吸収
- **ゼロアロケーション＆OS リアルタイム優先度**
  - WASAPI (Windows) / CoreAudio (macOS) コールバックは完全ロックフリー（`ringbuf`）、推論スレッドは Windows MMCSS（`Pro Audio`）/ macOS Time-Constraint 優先度で動作
- **ミニマル UI（墨ダーク / 和紙ライト切替対応）＆ Discord 風リアルタイムマイクテスト**
  - 仮想マイク未導入でも、`▶` ワンクリック＋縦スリット音量バー・出力音量ゲージ・**「ノイズ除去 / 生音声」即時切替**で自分の声と除去効果を確認可能
- **ライセンスクリーンなワンクリック仮想マイク導入**
  - **Windows**: アプリ内に他社ドライバをバンドルせず、ボタン 1 つで公式 WHQL 署名済み VB-CABLE (Donationware by VB-Audio: https://vb-audio.com) を一時取得・UAC 昇格サイレント導入
  - **macOS**: スクラッチ実装の **MIT ライセンス CoreAudio HAL ループバックドライバ（`Shizuka Virtual Mic`）** を Universal（`arm64` + `x86_64`）事前ビルド済みでアプリ内に同梱しており、**Xcode Command Line Tools 不要**でアプリ内ボタンから再起動なしに即時導入可能
- **アプリ内ワンクリック自動アップデート（Ed25519 署名検証）**
  - 起動時にバックグラウンドで GitHub Releases の最新版を静かにチェックし、新バージョンがあれば設定画面からワンクリックでダウンロード・検証・適用・自動再起動を実行（Windows ポータブル版では Releases ページへの案内ボタンに自動切替）

---

## 会議アプリ（Discord / Zoom / Teams / Meet 等）との連携セットアップ

### Windows の場合
1. **Shizuka** を起動し、仮想マイク未導入の案内バナーにある **「仮想マイクドライバを自動インストール」** をクリックして管理者確認（UAC）で「はい」を選択します。
2. 導入完了後、出力先に `CABLE Input` が自動選択されます（※ 初回導入時のみ Windows の再起動が推奨されます）。
3. 会議アプリのマイク入力設定で **`CABLE Output (VB-Audio Virtual Cable)`** を選択します。

### macOS の場合
1. **Shizuka** を起動し、案内バナーの **「Shizuka Virtual Mic (MIT) を導入」** をクリックします（事前ビルド済み Universal ドライバを同梱しているため **Xcode Command Line Tools 不要**、管理者パスワード入力後に `coreaudiod` が自動リロードされ即時認識されます。既存の `BlackHole 2ch` がある場合も自動対応します）。
2. 会議アプリのマイク入力設定で **`Shizuka Virtual Mic`** を選択します。
3. 初回起動時は「システム設定 → プライバシーとセキュリティ → マイク」で Shizuka にマイク権限を許可してください。

---

## 開発・ビルド手順 (Windows / macOS 共通)

必要環境: **Rust (stable)**, **Node.js (v20+)**
（※ Windows では Visual Studio C++ Build Tools、macOS では Xcode Command Line Tools `xcode-select --install`）

```bash
# リポジトリの取得
git clone https://github.com/seedling-nae/shizuka.git
cd shizuka

# 1. フロントエンド依存パッケージのインストール
npm install

# 2. 開発モードで起動
npm run tauri dev

# 3. リリース用ネイティブアプリのビルド (.exe / .msi / .app / .dmg)
npm run tauri build
```

## ロードマップ

`v0.1.0-beta` から `v1.0.0` 正式版までの段階リリース計画・進捗は **[ROADMAP.md](./ROADMAP.md)** をご覧ください。

## ライセンス
[MIT License](./LICENSE)
