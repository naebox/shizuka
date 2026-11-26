# 更新履歴 / Changelog

**Shizuka** の各バージョンにおける主な新機能・改善・不具合修正の記録です（日本語・英語併記）。  
Release history of key features, improvements, and bug fixes across **Shizuka** versions (bilingual Japanese & English).  
新しいバージョンをリリースする際は、必ず本ファイルに該当バージョンのセクション（`## [X.Y.Z]`、`### 主な変更点` および `### What's Changed`）を追記します（CI およびリリースワークフローで自動検証・GitHub Releases へ自動反映されます）。

---

## [0.7.0] (2026-11-26) — 英語表示対応＆アプリ内更新履歴ビューア / English Localization & In-App Release Notes

### 主な変更点
- **日本語・英語の多言語表示（i18n）に対応**
  - 初回起動時に OS の表示言語を自動判定（日本語環境は日本語、それ以外は英語）して表示します。
  - 設定画面（右上の歯車アイコン →「一般」）に「表示言語」（`自動` / `日本語` / `English`）を追加し、アプリを再起動することなく画面・通知領域／メニューバーの項目・ツールチップを即座に切り替えられるようにしました。
- **アプリ内で確認できる「更新履歴」ビューアと新着アップデート内容の表示を追加**
  - 設定画面の「アップデート」から、現在および過去バージョンの変更履歴を選択中の言語（日本語／英語）でそのまま閲覧できるようになりました（`GitHub Releases` を開くボタンも併設）。
  - 新しいバージョンが配信されている際は、アップデート適用ボタンの上にそのバージョンの主な変更点をわかりやすく表示します。
- **OS 権限ダイアログ・エラー案内・ドキュメントの英語対応**
  - macOS のマイクおよびシステム音声録音の権限確認ダイアログ、仮想マイク導入案内、ショートカット・自動更新の各種エラーメッセージを日英両言語に対応させました。
  - 英語版の説明書（[README.en.md](./README.en.md)）を追加し、変更履歴および GitHub Releases の案内を日英併記に統一しました。

### What's Changed
- **Full English & Japanese Localization (i18n)**
  - Automatically detects your OS display language on startup (Japanese on `ja*` systems, English everywhere else).
  - Added a one-click **Language** selector (`Auto` / `日本語` / `English`) under **Settings → General** that updates the entire window, system tray / menu bar items, and tooltips immediately without restarting.
- **In-App Release Notes Viewer & Update Preview**
  - Added a **Release Notes** viewer under **Settings → Updates** so you can browse current and historical release notes directly inside the app in your selected language (`ja` / `en`), alongside a quick button to open **GitHub Releases**.
  - When a new update is available, its localized highlights are now previewed right inside the update card before installing.
- **Localized macOS Permission Prompts, Setup Messages & Documentation**
  - Localized macOS Microphone and System Audio Recording permission dialogs (`InfoPlist.strings`), virtual driver setup messages, and all shortcut, audio stream, and updater notifications.
  - Added full English documentation ([README.en.md](./README.en.md)) and unified `CHANGELOG.md` and GitHub Releases notes into a bilingual format.

---

## [0.6.0] (2026-11-26) — 周囲の話し声カット（声の自動学習＆本人以外の声の消音） / Automatic Voice Isolation

### 主な変更点
- **新機能「周囲の話し声カット」を追加**
  - スイッチを ON にして普段通り話すだけで、会話の中からあなたの声の特徴を自動で学習・記憶します（事前の録音作業は不要です）。
  - あなたの話し始めは頭切れなく自然に通しつつ、家族の会話やテレビの話し声など「自分以外の声」が入ってきた時は約 0.3 秒で自動消音します。
  - 「えー」「うーん」などの伸ばす相槌や柔らかい語尾でも途切れにくい判定ロックと、声の「覚え直し」ボタン・「カットの厳しさ」スライダーを備えています。
- **声識別処理のバックグラウンド分離と低遅延化**
  - 声の識別モデル（Apache-2.0 `CAM++`・FP16 圧縮同梱）を低優先度の専用スレッドへ分離し、リアルタイム音声処理やスピーカーでの「マイクテスト」再生に遅れ・反響が生じないよう最適化しました。

### What's Changed
- **New Feature: Automatic Voice Isolation**
  - Simply turn on **Voice Isolation** and talk naturally—Shizuka learns and remembers your voiceprint automatically during everyday conversation with no manual enrollment phrases required.
  - Passes speech onsets cleanly without clipping your first syllable while muting nearby voices, family chatter, or TV dialogue automatically within ~0.3 seconds.
  - Includes turn-level speaker lock and phonetic-spread compensation so sustained filler vowels and soft endings never drop out, along with a **Re-learn** button and **Isolation Strictness** slider.
- **Dedicated Background Speaker Thread for Zero Audio Glitching**
  - Isolated the bundled FP16-compressed Apache-2.0 `CAM++` speaker model onto a low-priority background worker thread so real-time 10 ms audio processing and live **Mic Test** monitoring stay ultra-low-latency and echo-free.

---

## [0.5.0] (2026-11-25) — スピーカー通話向けのエコー除去＆画面表記の刷新 / Echo Cancellation & Clearer UI

### 主な変更点
- **新機能「エコー除去」（`WebRTC AEC3`）を追加**
  - イヤホンやヘッドセットを使わずスピーカーで通話しているとき、スピーカーから出る相手の声やシステム音をマイクが拾ってしまい相手に跳ね返る現象（エコー）を抑えます（Windows WASAPI ループバック／macOS 14.2 以降 CoreAudio Process Tap 対応）。
  - 物理マイクごとにエコー除去の ON / OFF を自動記憶し、アプリ内の「マイクテスト」実行中は自分の声が打ち消されないよう自動で一時バイパスします。
- **クロック同期補正の安定化**
  - 入力マイクと出力先のクロック差を吸収する適応リサンプラ（`DriftController`）の過渡応答を見直し、長時間の通話でも音程の揺れや遅延増加が起きないよう改善しました。
- **わかりやすい日本語表現への統一**
  - アプリ内の各機能名・説明文およびドキュメントを、専門用語に頼らない平易な日本語（「低音ノイズカット」「音量の自動調整」「話していない時は自動ミュート」など）へ刷新しました。

### What's Changed
- **New Feature: Speaker Echo Cancellation (`WebRTC AEC3`)**
  - Prevents remote voices and system audio coming from your speakers from feeding back into your microphone during speakerphone calls (powered by Windows WASAPI Loopback and macOS 14.2+ CoreAudio Process Tap).
  - Remembers Echo Cancellation state per physical microphone and automatically pauses cancellation while **Mic Test** is active so your monitored voice is never self-cancelled.
- **Critically Damped Clock Drift Compensation**
  - Refined the PI `DriftController` transient response between independent microphone and virtual-cable clocks to eliminate pitch warble and latency buildup over multi-hour calls.
- **Clearer In-App Labels & Documentation**
  - Simplified feature titles, hints, and documentation to make audio controls immediately understandable without audio-engineering jargon.

---

## [0.4.4] (2026-11-24) — Windows デバイス名認識の互換性修正 / Windows Device Name Compatibility Fix

### 主な変更点
- **Windows のフルデバイス名表示・一致判定を修正**
  - `cpal 0.17` 環境の Windows でオーディオデバイス名が短縮名になり、以前保存した設定と一致せず「マイク再接続待ち」になる場合があった問題を修正しました（短縮名で保存された設定からの自動引き継ぎにも対応）。

### What's Changed
- **Fixed Full Friendly Device Names & Matching on Windows**
  - Resolved an issue on Windows (`cpal 0.17` WASAPI) where audio endpoints resolved to generic short names instead of full hardware friendly names, preventing false "Waiting for Microphone" states and preserving backward compatibility with previously saved device selections.

---

## [0.4.3] (2026-11-24) — 小さな声・離れたマイクでの集音性を改善 / Quiet & Distant Microphone Sensitivity Fix

### 主な変更点
- **低音量マイク向けの内部レベル自動適正化（`PreDfnNormalizer`）を追加**
  - ダイナミックマイクや少し離れた位置から話した際、AI ノイズ除去が小さな声を雑音と誤認して消してしまう現象を解消しました（最終的な出力音量は変えずに内部の動作レベルのみを適正化します）。
- **自動ミュート・音量自動調整の低音量しきい値を緩和**
  - 小さめの話し声でも語尾が欠けたり途切れたりしないよう、無音判定と自動ゲインの動作下限を見直しました。

### What's Changed
- **Lookahead-Aligned Internal Level Normalization (`PreDfnNormalizer`)**
  - Eliminated `DeepFilterNet3`'s low-level sensitivity dead zone on quiet dynamic or distant microphones by smoothly driving internal inference into the optimal level window while restoring exact unity output gain.
- **Relaxed Low-Level Thresholds for Voice Gate & Auto Gain**
  - Tuned VAD noise gate hysteresis and AGC speech-floor thresholds so quiet voices and trailing syllables are never clipped.

---

## [0.4.2] (2026-11-24) — macOS 仮想マイク導入修正・動作負荷の軽減・アイコン更新改善 / macOS Driver Setup & Performance Improvements

### 主な変更点
- **macOS 仮想マイク（`Shizuka Virtual Mic`）のワンクリック導入を改善**
  - macOS Sonoma 14.4 以降で音声サービスの再読み込みが失敗する場合があった問題を修正し、権限設定とエラー診断ログを強化しました。
- **ストリーム切替時の安定性と 24-bit 機器対応**
  - モデルやデバイスの切り替え時に古い音声ストリームが残らないよう停止処理を改善し、24-bit オーディオインターフェースの入出力に対応しました。
- **CPU 負荷の最適化と Windows アイコン表示の改善**
  - リリースビルドの最適化（LTO）有効化と待機処理の軽量化に加え、Windows でのアップデート後にタスクバー等のアイコンがすぐ更新されるよう改善しました。

### What's Changed
- **Improved One-Click `Shizuka Virtual Mic` Setup on macOS**
  - Fixed `coreaudiod` reload reliability on macOS Sonoma 14.4+ and strengthened file permissions and diagnostic error reporting.
- **Cleaner Stream Teardown & 24-Bit Audio Interface Support**
  - Upgraded to `cpal 0.17`, ensured clean pause-before-drop teardown when switching models or devices, and added native 24-bit (`I24` / `U24`) audio stream support.
- **Lower CPU Usage & Windows Shell Icon Cache Refresh**
  - Enabled release LTO optimizations, reduced idle worker wakeups, and added NSIS post-install shell icon cache refresh hooks on Windows.

---

## [0.4.1] (2026-11-21) — アプリアイコン刷新・省電力化・安定性強化 / New App Icon, Power Efficiency & Hardening

### 主な変更点
- **新しいアプリアイコンへの刷新**
  - アプリアイコンおよび通知領域／メニューバーのアイコンを、「雑音の波形が静かな声へ整う」モチーフのデザインへ一新しました。
- **バックグラウンド常駐時の省電力化と設定保存の軽量化**
  - ウィンドウを閉じて通知領域／メニューバーに常駐している間は画面更新用の通信を完全停止し、スライダー操作時の設定保存もバックグラウンド処理化して動作を軽快にしました。
- **長時間稼働・再接続・終了処理の堅牢化**
  - 複数チャンネル搭載オーディオインターフェースでの単一マイク音量維持、未接続デバイスの明示表示、macOS 仮想マイク停止時の残響防止、およびアプリ終了・更新時の安全な停止処理と診断情報コピー機能を強化しました。

### What's Changed
- **Refreshed Application & Tray Icons**
  - Introduced the new Shizuka emblem depicting noisy waveforms calming into a clean, steady line across app and tray/menu bar icons.
- **Zero Background Polling & Non-Blocking Config Persistence**
  - Paused 20 Hz UI telemetry polling completely whenever the window is hidden to the tray/menu bar, and moved `settings.json` writes onto a debounced background thread.
- **Long-Session Reliability & Multi-Channel Interface Fixes**
  - Preserved unity mic gain on multi-channel XLR interfaces, displayed `(Disconnected)` states explicitly, eliminated residual ring-buffer audio in the macOS HAL driver, and hardened clean shutdown and diagnostics export.

---

## [0.4.0] (2026-11-21) — macOS 仮想マイク事前同梱＆ワンクリック自動アップデート / Bundled macOS Driver & Auto-Updater

### 主な変更点
- **macOS 仮想マイクドライバの事前ビルド同梱**
  - 追加の開発ツール（Xcode Command Line Tools）が入っていない Mac でも、アプリ内のボタンからワンクリック・再起動不要で `Shizuka Virtual Mic` を導入できるようになりました。
- **アプリ内ワンクリック自動アップデート機能を追加**
  - 設定画面から最新バージョンを確認し、電子署名を検証したうえでワンクリックでアップデートできるようになりました（ポータブル版は配布ページへの案内を表示します）。
- **ステレオ和音の操作音と CI / リリース自動検証の整備**
  - ショートカット操作時の効果音を聞き心地のよい和音へ刷新し、チェックサム（`SHA256SUMS.txt`）の自動生成とマルチプラットフォーム自動検証を整備しました。

### What's Changed
- **Prebuilt Universal `Shizuka Virtual Mic` Bundled on macOS**
  - Precompiled the Universal (`arm64` + `x86_64`) MIT CoreAudio HAL driver into macOS release bundles so users can install `Shizuka Virtual Mic` in one click without Xcode Command Line Tools or rebooting.
- **Signed One-Click In-App Auto-Updates**
  - Added Ed25519-verified in-app update checks and one-click installation from Settings (with a direct GitHub Releases link for portable builds).
- **Stereo Harmonic Shortcut Chimes & Automated CI/Release Verification**
  - Refined shortcut feedback cues with subtle stereo harmonic blooms and added automated multi-platform CI checks and `SHA256SUMS.txt` release checksums.

---

## [0.3.0] (2026-11-21) — 音質サポート機能（低音カット・音量自動調整・マイク別記憶）＆ショートカット / Voice Conditioning & Global Shortcuts

### 主な変更点
- **低音ノイズカット（65Hz ハイパスフィルタ）と AI 音質チューニング**
  - 声の自然な響きを保ちながら、机やマイクアームの振動音・息が当たる低い雑音を軽減します。あわせて高音質マイクでの不自然な音量変化や機械音を解消しました。
- **音量の自動調整（オートゲイン）と音割れ（`CLIP`）表示**
  - 話している間だけ聞き取りやすい音量へ自動でそろえ、黙っている間の周囲の雑音は持ち上げない設計にしました。入力が大きすぎて音割れしそうな時は `CLIP` バッジでお知らせします。
- **マイクごとの設定自動記憶とグローバルショートカット**
  - マイクを切り替えた際にマイクごとの調整値を自動で保存・復元する機能と、他のアプリを操作中でも使える「ノイズ除去 ON / OFF」「マイクミュート」のショートカットキー登録機能を追加しました。

### What's Changed
- **65 Hz Low-Cut Filter & Neural Suppression Tuning**
  - Added a zero-allocation Butterworth high-pass filter to cut desk rumble and plosives while keeping vocal warmth intact, and eliminated mid-speech metallic artifacts on high-SNR microphones.
- **VAD-Gated Auto Gain Control (`AGC`) & Clipping Indicator**
  - Levels active speech smoothly without boosting background noise during pauses, paired with a live `CLIP` warning badge when peaks reach the limiter.
- **Per-Microphone Profiles & Global Hotkeys**
  - Automatically remembers DSP settings for each physical microphone and adds customizable system-wide shortcuts (with mic mute and optional sound cues).

---

## [0.2.0] (2026-11-21) — 自動再接続・OS 自動起動・常駐基盤の強化 / Auto-Reconnect, Launch at Login & Tray Foundation

### 主な変更点
- **マイク抜き差し・スリープ復帰時の自動再接続**
  - USB マイクの一時的な切断やスリープ復帰時に自動で再接続し、「システム既定のマイク」選択時は OS 側の既定デバイス変更にも自動追従するよう改善しました。
- **OS ログイン時の自動起動と多重起動防止**
  - PC 起動時にウィンドウを出さず通知領域／メニューバーへ静かに常駐する自動起動設定と、アプリの二重起動防止を追加しました。
- **設定モーダル画面と診断情報コピーの追加**
  - 右上の歯車ボタンから開く設定画面を新設し、自動起動の切り替えや動作状態の確認・診断情報のコピーを行えるようにしました。

### What's Changed
- **Automatic USB Reconnect & Sleep/Wake Recovery**
  - Added stream watchdog recovery for unplugged USB mics and OS sleep/wake transitions, plus automatic tracking when using the OS default microphone.
- **Launch in Background at Login & Single-Instance Guard**
  - Added quiet minimized tray/menu bar autostart on OS login and prevented duplicate app instances.
- **Settings Modal & Diagnostics Copy**
  - Added the Settings modal dialog (`<dialog>`) with autostart controls, live pipeline health counters, and one-click diagnostics copy.

---

## [0.1.0-beta] (2026-11-20) — 初回ベータ版リリース / Initial Beta Release

### 主な変更点
- **端末内で完結するリアルタイム AI マイクノイズキャンセリング（`DeepFilterNet3`）**
  - 「高精度モード」と「超低遅延モード」の切り替え、話していない時の自動ミュート（VAD ゲート）、長時間通話向けのクロック同期補正を実装しました。
- **その場で聞き比べできるマイクテストとワンクリック仮想マイク導入**
  - 「ノイズ除去後」と「生音声」を切り替えられるマイクテスト機能、および Windows（VB-CABLE 公式インストーラ連携）・macOS（MIT ライセンス `Shizuka Virtual Mic`）の仮想マイク導入機能を搭載しました。

### What's Changed
- **On-Device Real-Time AI Microphone Noise Cancellation (`DeepFilterNet3`)**
  - Implemented 48 kHz neural noise suppression with Standard and Low-Latency modes, an auto-mute VAD noise gate, and adaptive clock-drift compensation.
- **Live A/B Mic Test & One-Click Virtual Microphone Setup**
  - Added real-time denoised vs. raw voice monitoring and one-click virtual audio driver setup on Windows (official VB-CABLE integration) and macOS (clean-room MIT `Shizuka Virtual Mic`).
