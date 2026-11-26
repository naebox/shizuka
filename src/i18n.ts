import type { Locale } from "./types";

export function detectBrowserLocale(): Locale {
  if (typeof navigator !== "undefined") {
    const primary = (navigator.language || "").trim().toLowerCase();
    if (primary === "ja" || primary.startsWith("ja-") || primary.startsWith("ja_")) {
      return "ja";
    }
  }
  return "en";
}

export interface UiMessages {
  docTitle: string;
  topbar: {
    switchToLight: string;
    switchToDark: string;
    rescanDevices: string;
    openSettings: string;
    openSettingsWithUpdate: string;
  };
  banners: {
    waitingMicPrefix: string;
    waitingNamedMic: (name: string) => string;
    waitingAnyMic: string;
    reconnectingStream: string;
  };
  hero: {
    title: string;
    stateOn: string;
    stateBypass: string;
    micMutedLabel: string;
    micLiveLabel: string;
    unmuteBtn: string;
    muteBtn: string;
    meterInputRaw: string;
    meterMutedSilence: string;
    meterDenoised: string;
    meterBypassRaw: string;
    meterMutedAria: string;
    clipTooltip: string;
  };
  routing: {
    sectionTitle: string;
    statusVirtualActive: string;
    statusStandaloneTest: string;
    inputMicLabel: string;
    defaultMicOption: string;
    disconnectedSuffix: string;
    virtualOutLabel: string;
    noVirtualOutOption: string;
    recommendedVirtualPrefix: string;
    driverMissingTitle: string;
    driverSpinnerAria: string;
    driverInstallingBtn: string;
    driverInstallMacBtn: string;
    driverInstallWinBtn: string;
  };
  tuning: {
    sectionTitle: string;
    aiModelLegend: string;
    modelStandard: string;
    modelLowLatency: string;
    presetLegend: string;
    presetNatural: string;
    presetBalanced: string;
    presetMax: string;
    attenLabel: string;
    attenUnlimited: string;
    inputGainLabel: string;
    hpfTitle: string;
    hpfHint: string;
    agcTitle: string;
    agcHint: string;
    aecTitle: string;
    aecHint: string;
    aecPausedInTest: string;
    aecActiveWithErle: (erleDb: string) => string;
    aecActive: string;
    aecStandby: string;
    aecSpeakerLabel: string;
    aecDefaultSpeakerOption: string;
    aecMacPermissionHint: string;
    vadTitle: string;
    vadStandby: string;
    vadSpeaking: string;
    vadMuted: string;
    vadSliderLabel: string;
    vadSliderHint: string;
    voiceGateTitle: string;
    voiceGateHint: string;
    voiceGateLearningPct: (pct: number) => string;
    voiceGateLearning: string;
    voiceGateBlocking: string;
    voiceEnrolledStatus: string;
    voiceRelearnBtn: string;
    voiceAutoLearnPrompt: string;
    voiceAutoLearnHint: string;
    voiceStrictnessLabel: string;
    voiceStrictnessRelaxed: (pct: number) => string;
    voiceStrictnessBalanced: (pct: number) => string;
    voiceStrictnessStrict: (pct: number) => string;
    voiceStrictnessHint: string;
  };
  micTest: {
    sectionTitle: string;
    testingBadge: string;
    stopAria: string;
    startAria: string;
    meterLabel: string;
    stoppedAria: string;
    modeLegend: string;
    modeDenoised: string;
    modeRaw: string;
    volumeLabel: string;
    speakerLabel: string;
    defaultSpeakerOption: string;
  };
  footer: {
    closeRunsInBackground: string;
    quitBtn: string;
  };
  settings: {
    title: string;
    closeAria: string;
    generalGroup: string;
    languageLegend: string;
    languageAuto: string;
    autostartTitle: string;
    shortcutsGroup: string;
    unassigned: string;
    shortcutToggleNc: string;
    shortcutToggleMute: string;
    recordingPrompt: string;
    recordBtn: string;
    recordNcContext: string;
    recordMuteContext: string;
    clearNcAria: string;
    clearMuteAria: string;
    shortcutSoundTitle: string;
    shortcutSoundHint: (isMac: boolean) => string;
    diagnosticsGroup: string;
    diagPipelineStatus: string;
    diagStateRunning: string;
    diagStateWaiting: string;
    diagStateReconnecting: string;
    diagStateStopped: string;
    diagClockSync: string;
    diagReconnects: string;
    diagReconnectValue: (count: number) => string;
    diagSampleRate: string;
    diagInputDrops: string;
    diagOutputUnderruns: string;
    diagCopiedBtn: string;
    diagCopyBtn: string;
    updatesGroup: string;
    updateChecking: string;
    updateAvailable: (version: string, isPortable: boolean) => string;
    updateUpToDate: string;
    updateCheckPrompt: string;
    updateCheckingSpinnerAria: string;
    updateCheckBtn: string;
    updateInstallingProgress: string;
    updateDownloadingProgress: string;
    updatePortableHint: string;
    openReleasesBtn: string;
    updateApplyingBtn: string;
    updateNowBtn: (version: string) => string;
    whatsNewHeading: (version: string) => string;
    releaseNotesBtn: string;
    githubReleasesBtn: string;
    openGithubReleasesAria: string;
    releaseNotesTitle: string;
    backToSettingsAria: string;
    badgeAvailable: string;
    badgeCurrent: string;
  };
  errors: {
    unsupportedKey: string;
    unmodifiedKeyNotAllowed: string;
    loadDevicesFailed: (detail: string) => string;
    toggleNcFailed: (detail: string) => string;
    toggleMuteFailed: (detail: string) => string;
    switchDeviceFailed: (detail: string) => string;
    switchModelFailed: (detail: string) => string;
    updateDspFailed: (detail: string) => string;
    switchSpeakerFailed: (detail: string) => string;
    updateVoiceGateFailed: (detail: string) => string;
    resetVoiceProfileFailed: (detail: string) => string;
    toggleMicTestFailed: (detail: string) => string;
    switchMonitorModeFailed: (detail: string) => string;
    changeMonitorVolFailed: (detail: string) => string;
    quitFailed: (detail: string) => string;
    copyClipboardFailed: (detail: string) => string;
    switchLanguageFailed: (detail: string) => string;
  };
}

const JA_MESSAGES: UiMessages = {
  docTitle: "Shizuka — AI マイクノイズキャンセリング",
  topbar: {
    switchToLight: "ライトモードに切り替え",
    switchToDark: "ダークモードに切り替え",
    rescanDevices: "オーディオデバイスを再スキャン",
    openSettings: "アプリ設定を開く",
    openSettingsWithUpdate: "アプリ設定を開く（新しいアップデートがあります）",
  },
  banners: {
    waitingMicPrefix: "マイク再接続待ち：",
    waitingNamedMic: (name) => `「${name}」が接続されると自動で再開します。`,
    waitingAnyMic: "マイクが接続されると自動で再開します。",
    reconnectingStream: "オーディオストリームを自動復旧しています...",
  },
  hero: {
    title: "AI ノイズキャンセリング",
    stateOn: "ON",
    stateBypass: "スルー",
    micMutedLabel: "マイクミュート中（無音）",
    micLiveLabel: "マイク送信中",
    unmuteBtn: "ミュート解除",
    muteBtn: "ミュート",
    meterInputRaw: "入力マイク (処理前)",
    meterMutedSilence: "ミュート中 (無音送信)",
    meterDenoised: "ノイズ除去後",
    meterBypassRaw: "スルー出力 (未処理)",
    meterMutedAria: "ミュート",
    clipTooltip: "入力または出力がリミッターに達しました。入力ゲインを下げてください。",
  },
  routing: {
    sectionTitle: "入出力デバイス設定",
    statusVirtualActive: "仮想マイク出力中",
    statusStandaloneTest: "単体テストモード",
    inputMicLabel: "使用する物理マイク",
    defaultMicOption: "システム既定のマイク (自動選択)",
    disconnectedSuffix: "(未接続)",
    virtualOutLabel: "仮想マイク出力先 (会議アプリ連携)",
    noVirtualOutOption: "出力しない (マイクテストのみ)",
    recommendedVirtualPrefix: "★推奨 [仮想] ",
    driverMissingTitle: "仮想マイクが未導入です（会議アプリ連携用）：",
    driverSpinnerAria: "ドライバをセットアップ中",
    driverInstallingBtn: "公式ドライバを取得・セットアップ中...",
    driverInstallMacBtn: "Shizuka Virtual Mic (MIT) を導入",
    driverInstallWinBtn: "仮想マイクドライバを自動インストール",
  },
  tuning: {
    sectionTitle: "除去強度・音質チューニング",
    aiModelLegend: "AI モデル選択",
    modelStandard: "高精度 DFN3 (推奨)",
    modelLowLatency: "超低遅延 DFN3-LL",
    presetLegend: "ノイズ除去プリセット",
    presetNatural: "ナチュラル",
    presetBalanced: "バランス (推奨)",
    presetMax: "最強 (打鍵音)",
    attenLabel: "最大ノイズ減衰量",
    attenUnlimited: "無制限 (100 dB)",
    inputGainLabel: "マイク入力ゲイン補正",
    hpfTitle: "低音ノイズカット",
    hpfHint: "机やアームの振動、息が当たる音などの低い雑音を軽減",
    agcTitle: "音量の自動調整",
    agcHint: "話している間だけ、声の大きさを聞き取りやすい音量にそろえます",
    aecTitle: "エコー除去",
    aecHint: "スピーカーから聞こえる声や音をマイクが拾うのを防止",
    aecPausedInTest: "テスト中停止",
    aecActiveWithErle: (erleDb) => `稼働中 · -${erleDb} dB`,
    aecActive: "稼働中",
    aecStandby: "待機中",
    aecSpeakerLabel: "使用中のスピーカー",
    aecDefaultSpeakerOption: "システム既定のスピーカー (自動選択)",
    aecMacPermissionHint:
      "初回有効化時のみ macOS のシステム音声録音の許可が必要です（macOS 14.2+）",
    vadTitle: "話していない時は自動ミュート",
    vadStandby: "待機",
    vadSpeaking: "話し中",
    vadMuted: "消音中",
    vadSliderLabel: "消音の厳しさ",
    vadSliderHint:
      "上げるほど、話していない時のキーボード音や周囲の音をしっかり消します",
    voiceGateTitle: "周囲の話し声カット",
    voiceGateHint:
      "普段の会話からあなたの声を自動で覚え、家族の声やテレビの話し声を消音します",
    voiceGateLearningPct: (pct) => `学習中 ${pct}%`,
    voiceGateLearning: "声を学習中",
    voiceGateBlocking: "周囲の声を消音中",
    voiceEnrolledStatus: "あなたの声を記憶済み",
    voiceRelearnBtn: "覚え直す",
    voiceAutoLearnPrompt: "普段通りに話すと自動で声を覚えます",
    voiceAutoLearnHint:
      "マイクの近くで数秒間会話すると記憶が完了します（話している間だけ進みます）",
    voiceStrictnessLabel: "カットの厳しさ",
    voiceStrictnessRelaxed: (pct) => `ゆるめ (${pct}%)`,
    voiceStrictnessBalanced: (pct) => `標準 (${pct}%)`,
    voiceStrictnessStrict: (pct) => `厳しめ (${pct}%)`,
    voiceStrictnessHint:
      "かすれ声などで自分の声が途切れる時は下げ、周囲の話し声をより強く消したい時は上げます",
  },
  micTest: {
    sectionTitle: "マイクテスト",
    testingBadge: "テスト中",
    stopAria: "マイクテストを停止",
    startAria: "マイクテストを開始",
    meterLabel: "マイクテスト音量レベル",
    stoppedAria: "停止中",
    modeLegend: "マイクテストの音声モード",
    modeDenoised: "ノイズ除去",
    modeRaw: "生音声",
    volumeLabel: "マイクテスト出力音量",
    speakerLabel: "テスト再生用スピーカー / ヘッドホン",
    defaultSpeakerOption: "既定のスピーカー / ヘッドホン",
  },
  footer: {
    closeRunsInBackground: "[×] でバックグラウンド動作",
    quitBtn: "アプリを終了",
  },
  settings: {
    title: "アプリ設定",
    closeAria: "アプリ設定を閉じる",
    generalGroup: "一般",
    languageLegend: "表示言語",
    languageAuto: "自動",
    autostartTitle: "OS 起動時にバックグラウンドで自動起動",
    shortcutsGroup: "グローバルショートカット・操作音",
    unassigned: "未割当",
    shortcutToggleNc: "ノイズ除去 ON / OFF",
    shortcutToggleMute: "マイクミュート",
    recordingPrompt: "キーを入力 (Esc取消)",
    recordBtn: "設定",
    recordNcContext: "（ノイズ除去 ON/OFF）",
    recordMuteContext: "（マイクミュート）",
    clearNcAria: "ノイズ除去 ON/OFF のショートカットを解除",
    clearMuteAria: "マイクミュートのショートカットを解除",
    shortcutSoundTitle: "ショートカット操作音",
    shortcutSoundHint: (isMac) =>
      `ホットキーや${isMac ? "メニューバー" : "通知領域"}からの切替時に控えめな確認音を鳴らします`,
    diagnosticsGroup: "自動復旧・診断情報",
    diagPipelineStatus: "パイプライン状態",
    diagStateRunning: "稼働中",
    diagStateWaiting: "再接続待ち",
    diagStateReconnecting: "再接続中",
    diagStateStopped: "停止中",
    diagClockSync: "クロック同期補正",
    diagReconnects: "自動復旧回数",
    diagReconnectValue: (count) => `${count} 回`,
    diagSampleRate: "サンプルレート",
    diagInputDrops: "入力ドロップ",
    diagOutputUnderruns: "出力アンダーラン",
    diagCopiedBtn: "診断情報をコピーしました",
    diagCopyBtn: "診断情報をコピー",
    updatesGroup: "アップデート",
    updateChecking: "最新バージョンを確認しています...",
    updateAvailable: (version, isPortable) =>
      `v${version} が利用可能です${isPortable ? "（ポータブル版）" : ""}`,
    updateUpToDate: "最新バージョンです",
    updateCheckPrompt: "最新版をオンラインで確認できます",
    updateCheckingSpinnerAria: "アップデートを確認中",
    updateCheckBtn: "更新を確認",
    updateInstallingProgress: "インストールを適用しています...",
    updateDownloadingProgress: "アップデートをダウンロード中...",
    updatePortableHint:
      "ポータブル版は自動上書きに対応していないため、GitHub Releases から最新版を取得してください。",
    openReleasesBtn: "Releases ページを開く",
    updateApplyingBtn: "アップデートを適用中...",
    updateNowBtn: (version) => `v${version} へ今すぐアップデート`,
    whatsNewHeading: (version) =>
      version ? `v${version} の主な変更点` : "主な変更点",
    releaseNotesBtn: "更新履歴を見る",
    githubReleasesBtn: "GitHub Releases",
    openGithubReleasesAria: "GitHub Releases をブラウザで開く",
    releaseNotesTitle: "更新履歴",
    backToSettingsAria: "アプリ設定に戻る",
    badgeAvailable: "新着",
    badgeCurrent: "現在",
  },
  errors: {
    unsupportedKey:
      "このキーはショートカットに割り当てできません。別のキーを押してください。",
    unmodifiedKeyNotAllowed:
      "単一キーはファンクションキー（F1〜F24 等）のみ登録できます。Ctrl / Alt / Shift 等と組み合わせてください。",
    loadDevicesFailed: (d) => `オーディオデバイスの読み込みに失敗しました: ${d}`,
    toggleNcFailed: (d) => `ノイズ除去の切替に失敗しました: ${d}`,
    toggleMuteFailed: (d) => `ミュート切替に失敗しました: ${d}`,
    switchDeviceFailed: (d) => `デバイスの切替に失敗しました: ${d}`,
    switchModelFailed: (d) => `AI モデルの切替に失敗しました: ${d}`,
    updateDspFailed: (d) => `音質パラメータの反映に失敗しました: ${d}`,
    switchSpeakerFailed: (d) => `スピーカーの切替に失敗しました: ${d}`,
    updateVoiceGateFailed: (d) => `周囲の話し声カットの反映に失敗しました: ${d}`,
    resetVoiceProfileFailed: (d) => `声の記憶のリセットに失敗しました: ${d}`,
    toggleMicTestFailed: (d) => `マイクテストの切替に失敗しました: ${d}`,
    switchMonitorModeFailed: (d) => `モニターモードの切替に失敗しました: ${d}`,
    changeMonitorVolFailed: (d) => `モニター音量の変更に失敗しました: ${d}`,
    quitFailed: (d) => `終了処理に失敗しました: ${d}`,
    copyClipboardFailed: (d) => `クリップボードへのコピーに失敗しました: ${d}`,
    switchLanguageFailed: (d) => `言語の切替に失敗しました: ${d}`,
  },
};

const EN_MESSAGES: UiMessages = {
  docTitle: "Shizuka — AI Microphone Noise Cancellation",
  topbar: {
    switchToLight: "Switch to light mode",
    switchToDark: "Switch to dark mode",
    rescanDevices: "Rescan audio devices",
    openSettings: "Open settings",
    openSettingsWithUpdate: "Open settings (new update available)",
  },
  banners: {
    waitingMicPrefix: "Waiting for Microphone: ",
    waitingNamedMic: (name) =>
      `Will resume automatically when "${name}" is reconnected.`,
    waitingAnyMic: "Will resume automatically when a microphone is connected.",
    reconnectingStream: "Reconnecting audio stream...",
  },
  hero: {
    title: "AI Noise Cancellation",
    stateOn: "ON",
    stateBypass: "Bypass",
    micMutedLabel: "Mic Muted (Silent)",
    micLiveLabel: "Mic Active",
    unmuteBtn: "Unmute",
    muteBtn: "Mute",
    meterInputRaw: "Mic Input (Raw)",
    meterMutedSilence: "Muted (Sending Silence)",
    meterDenoised: "Noise Cancelled",
    meterBypassRaw: "Bypass (Unprocessed)",
    meterMutedAria: "Muted",
    clipTooltip:
      "Input or output reached the limiter ceiling. Lower Mic Input Gain.",
  },
  routing: {
    sectionTitle: "Audio Devices",
    statusVirtualActive: "Virtual Mic Active",
    statusStandaloneTest: "Mic Test Only",
    inputMicLabel: "Input Microphone",
    defaultMicOption: "System Default Microphone (Auto)",
    disconnectedSuffix: "(Disconnected)",
    virtualOutLabel: "Virtual Mic Output (for Calls)",
    noVirtualOutOption: "Disabled (Mic Test Only)",
    recommendedVirtualPrefix: "★ Recommended [Virtual] ",
    driverMissingTitle: "Virtual Microphone Not Installed (for Call Apps):",
    driverSpinnerAria: "Installing virtual audio driver",
    driverInstallingBtn: "Setting up virtual driver...",
    driverInstallMacBtn: "Install Shizuka Virtual Mic (MIT)",
    driverInstallWinBtn: "Install Virtual Mic Driver",
  },
  tuning: {
    sectionTitle: "Noise & Voice Tuning",
    aiModelLegend: "AI Model",
    modelStandard: "Standard DFN3 (Rec.)",
    modelLowLatency: "Low-Latency DFN3-LL",
    presetLegend: "Suppression Preset",
    presetNatural: "Natural",
    presetBalanced: "Balanced",
    presetMax: "Maximum",
    attenLabel: "Max Noise Attenuation",
    attenUnlimited: "Unlimited (100 dB)",
    inputGainLabel: "Mic Input Gain",
    hpfTitle: "Low-Cut Filter",
    hpfHint: "Reduces desk rumble, mic arm vibrations, and breath plosives",
    agcTitle: "Auto Gain Control",
    agcHint: "Levels your speaking volume without boosting background noise",
    aecTitle: "Echo Cancellation",
    aecHint: "Prevents your mic from picking up voices and audio from speakers",
    aecPausedInTest: "Paused in Test",
    aecActiveWithErle: (erleDb) => `Active · -${erleDb} dB`,
    aecActive: "Active",
    aecStandby: "Standby",
    aecSpeakerLabel: "Reference Speaker",
    aecDefaultSpeakerOption: "System Default Speaker (Auto)",
    aecMacPermissionHint:
      "Requires macOS System Audio Recording permission on first use (macOS 14.2+)",
    vadTitle: "Auto-Mute Voice Gate",
    vadStandby: "Standby",
    vadSpeaking: "Speaking",
    vadMuted: "Gated",
    vadSliderLabel: "Gate Threshold",
    vadSliderHint:
      "Higher values silence typing and ambient noise more aggressively between phrases",
    voiceGateTitle: "Voice Isolation",
    voiceGateHint:
      "Automatically learns your voice as you talk and mutes nearby voices or TV audio",
    voiceGateLearningPct: (pct) => `Learning ${pct}%`,
    voiceGateLearning: "Learning Voice",
    voiceGateBlocking: "Muting Others",
    voiceEnrolledStatus: "Voice Profile Learned",
    voiceRelearnBtn: "Re-learn",
    voiceAutoLearnPrompt: "Speak naturally to learn your voice",
    voiceAutoLearnHint:
      "Talk near your mic for a few seconds to complete enrollment (advances while speaking)",
    voiceStrictnessLabel: "Isolation Strictness",
    voiceStrictnessRelaxed: (pct) => `Relaxed (${pct}%)`,
    voiceStrictnessBalanced: (pct) => `Balanced (${pct}%)`,
    voiceStrictnessStrict: (pct) => `Strict (${pct}%)`,
    voiceStrictnessHint:
      "Lower if soft speech cuts out; raise to reject background voices more strictly",
  },
  micTest: {
    sectionTitle: "Mic Test",
    testingBadge: "Testing",
    stopAria: "Stop mic test",
    startAria: "Start mic test",
    meterLabel: "Mic test level",
    stoppedAria: "Stopped",
    modeLegend: "Mic test audio mode",
    modeDenoised: "Noise Cancelled",
    modeRaw: "Raw Audio",
    volumeLabel: "Mic test playback volume",
    speakerLabel: "Playback Speaker / Headphones",
    defaultSpeakerOption: "Default Speaker / Headphones",
  },
  footer: {
    closeRunsInBackground: "[×] Minimizes to background",
    quitBtn: "Quit Shizuka",
  },
  settings: {
    title: "Settings",
    closeAria: "Close settings",
    generalGroup: "General",
    languageLegend: "Language",
    languageAuto: "Auto",
    autostartTitle: "Launch in background at login",
    shortcutsGroup: "Global Shortcuts & Sound",
    unassigned: "Unassigned",
    shortcutToggleNc: "Toggle Noise Cancellation",
    shortcutToggleMute: "Toggle Mic Mute",
    recordingPrompt: "Press keys (Esc)",
    recordBtn: "Record",
    recordNcContext: " (Toggle Noise Cancellation)",
    recordMuteContext: " (Toggle Mic Mute)",
    clearNcAria: "Clear Noise Cancellation shortcut",
    clearMuteAria: "Clear Mic Mute shortcut",
    shortcutSoundTitle: "Shortcut Sound Cue",
    shortcutSoundHint: (isMac) =>
      `Plays a subtle chime when toggling via hotkeys or the ${
        isMac ? "menu bar" : "system tray"
      }`,
    diagnosticsGroup: "Auto-Recovery & Diagnostics",
    diagPipelineStatus: "Pipeline Status",
    diagStateRunning: "Running",
    diagStateWaiting: "Waiting for Device",
    diagStateReconnecting: "Reconnecting",
    diagStateStopped: "Stopped",
    diagClockSync: "Clock Drift Sync",
    diagReconnects: "Auto-Reconnects",
    diagReconnectValue: (count) => `${count}`,
    diagSampleRate: "Sample Rate",
    diagInputDrops: "Input Drops",
    diagOutputUnderruns: "Output Underruns",
    diagCopiedBtn: "Diagnostics Copied",
    diagCopyBtn: "Copy Diagnostics",
    updatesGroup: "Updates",
    updateChecking: "Checking for updates...",
    updateAvailable: (version, isPortable) =>
      `v${version} is available${isPortable ? " (Portable)" : ""}`,
    updateUpToDate: "You're up to date",
    updateCheckPrompt: "Check online for the latest release",
    updateCheckingSpinnerAria: "Checking for updates",
    updateCheckBtn: "Check",
    updateInstallingProgress: "Installing update...",
    updateDownloadingProgress: "Downloading update...",
    updatePortableHint:
      "Portable builds cannot auto-update in place. Download the latest release from GitHub Releases.",
    openReleasesBtn: "Open Releases Page",
    updateApplyingBtn: "Applying update...",
    updateNowBtn: (version) => `Update to v${version} Now`,
    whatsNewHeading: (version) =>
      version ? `What's New in v${version}` : "What's New",
    releaseNotesBtn: "Release Notes",
    githubReleasesBtn: "GitHub Releases",
    openGithubReleasesAria: "Open GitHub Releases in browser",
    releaseNotesTitle: "Release Notes",
    backToSettingsAria: "Back to settings",
    badgeAvailable: "New",
    badgeCurrent: "Current",
  },
  errors: {
    unsupportedKey:
      "This key cannot be assigned as a shortcut. Please press another key.",
    unmodifiedKeyNotAllowed:
      "Single-key shortcuts are only allowed for function keys (F1–F24, etc.). Combine with Ctrl, Alt, or Shift.",
    loadDevicesFailed: (d) => `Failed to load audio devices: ${d}`,
    toggleNcFailed: (d) => `Failed to toggle noise cancellation: ${d}`,
    toggleMuteFailed: (d) => `Failed to toggle mic mute: ${d}`,
    switchDeviceFailed: (d) => `Failed to switch audio device: ${d}`,
    switchModelFailed: (d) => `Failed to switch AI model: ${d}`,
    updateDspFailed: (d) => `Failed to update audio parameters: ${d}`,
    switchSpeakerFailed: (d) => `Failed to switch reference speaker: ${d}`,
    updateVoiceGateFailed: (d) => `Failed to update Voice Isolation: ${d}`,
    resetVoiceProfileFailed: (d) => `Failed to reset voice profile: ${d}`,
    toggleMicTestFailed: (d) => `Failed to toggle Mic Test: ${d}`,
    switchMonitorModeFailed: (d) => `Failed to switch monitor mode: ${d}`,
    changeMonitorVolFailed: (d) => `Failed to change monitor volume: ${d}`,
    quitFailed: (d) => `Failed to quit application: ${d}`,
    copyClipboardFailed: (d) => `Failed to copy to clipboard: ${d}`,
    switchLanguageFailed: (d) => `Failed to switch language: ${d}`,
  },
};

export function getMessages(locale: Locale): UiMessages {
  return locale === "ja" ? JA_MESSAGES : EN_MESSAGES;
}
