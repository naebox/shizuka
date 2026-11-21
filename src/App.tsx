import {
  type CSSProperties,
  useCallback,
  useEffect,
  useId,
  useRef,
  useState,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  AudioLines,
  Check,
  CheckCircle2,
  ClipboardCopy,
  Download,
  Headphones,
  Keyboard,
  Mic,
  MicOff,
  Moon,
  Play,
  RefreshCw,
  Settings,
  Sliders,
  Square,
  Sun,
  TriangleAlert,
  Volume2,
  VolumeX,
  Waves,
  X,
} from "lucide-react";
import type {
  AppConfig,
  AudioTelemetry,
  DeviceListResponse,
  InstallDriverResult,
  ModelVariant,
  ShortcutAction,
} from "./types";

const supportsNativeSwitch =
  typeof HTMLInputElement !== "undefined" &&
  "switch" in HTMLInputElement.prototype;

const COLOR_SCHEME_STORAGE_KEY = "color-scheme";

const MODIFIER_CODES = new Set([
  "ControlLeft",
  "ControlRight",
  "ShiftLeft",
  "ShiftRight",
  "AltLeft",
  "AltRight",
  "MetaLeft",
  "MetaRight",
]);

const UNMODIFIED_ALLOWED_CODES = new Set([
  "F1",
  "F2",
  "F3",
  "F4",
  "F5",
  "F6",
  "F7",
  "F8",
  "F9",
  "F10",
  "F11",
  "F12",
  "F13",
  "F14",
  "F15",
  "F16",
  "F17",
  "F18",
  "F19",
  "F20",
  "F21",
  "F22",
  "F23",
  "F24",
  "Pause",
  "ScrollLock",
  "PrintScreen",
  "NumLock",
  "AudioVolumeMute",
  "MediaPlayPause",
  "MediaStop",
  "MediaTrackNext",
  "MediaTrackPrevious",
]);

const SUPPORTED_PRIMARY_CODES = new Set([
  ...UNMODIFIED_ALLOWED_CODES,
  "Space",
  "Minus",
  "Equal",
  "BracketLeft",
  "BracketRight",
  "Backslash",
  "Semicolon",
  "Quote",
  "Backquote",
  "Comma",
  "Period",
  "Slash",
  "Insert",
  "Home",
  "End",
  "PageUp",
  "PageDown",
  "ArrowUp",
  "ArrowDown",
  "ArrowLeft",
  "ArrowRight",
  "Numpad0",
  "Numpad1",
  "Numpad2",
  "Numpad3",
  "Numpad4",
  "Numpad5",
  "Numpad6",
  "Numpad7",
  "Numpad8",
  "Numpad9",
  "NumpadAdd",
  "NumpadSubtract",
  "NumpadMultiply",
  "NumpadDivide",
  "NumpadDecimal",
]);

function isSupportedPrimaryCode(code: string): boolean {
  return (
    /^Key[A-Z]$/.test(code) ||
    /^Digit[0-9]$/.test(code) ||
    SUPPORTED_PRIMARY_CODES.has(code)
  );
}

function buildAcceleratorFromKeyEvent(
  e: KeyboardEvent
): { ok: true; value: string } | { ok: false; error: string } | null {
  if (MODIFIER_CODES.has(e.code)) {
    return null;
  }
  if (!isSupportedPrimaryCode(e.code)) {
    return {
      ok: false,
      error: "このキーはショートカットに割り当てできません。別のキーを押してください。",
    };
  }
  const mods: string[] = [];
  if (e.ctrlKey) mods.push("Control");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");
  if (e.metaKey) mods.push("Super");

  if (mods.length === 0 && !UNMODIFIED_ALLOWED_CODES.has(e.code)) {
    return {
      ok: false,
      error:
        "単一キーはファンクションキー（F1〜F24 等）のみ登録できます。Ctrl / Alt / Shift 等と組み合わせてください。",
    };
  }

  return {
    ok: true,
    value: [...mods, e.code].join("+"),
  };
}

const KEY_DISPLAY_MAP: Record<string, string> = {
  Space: "Space",
  Minus: "-",
  Equal: "=",
  BracketLeft: "[",
  BracketRight: "]",
  Backslash: "\\",
  Semicolon: ";",
  Quote: "'",
  Backquote: "`",
  Comma: ",",
  Period: ".",
  Slash: "/",
  ArrowUp: "↑",
  ArrowDown: "↓",
  ArrowLeft: "←",
  ArrowRight: "→",
  NumpadAdd: "Num+",
  NumpadSubtract: "Num-",
  NumpadMultiply: "Num*",
  NumpadDivide: "Num/",
  NumpadDecimal: "Num.",
  AudioVolumeMute: "Mute",
  MediaPlayPause: "Play",
  MediaStop: "Stop",
  MediaTrackNext: "Next",
  MediaTrackPrevious: "Prev",
};

function formatShortcutLabel(
  accelerator: string | null | undefined,
  isMac: boolean
): string {
  if (!accelerator) return "未割当";
  const parts = accelerator.split("+").map((p) => p.trim());
  return parts
    .map((part) => {
      if (part === "Control" || part === "Ctrl") return isMac ? "⌃" : "Ctrl";
      if (part === "Alt") return isMac ? "⌥" : "Alt";
      if (part === "Shift") return isMac ? "⇧" : "Shift";
      if (part === "Super" || part === "Meta" || part === "Command") {
        return isMac ? "⌘" : "Win";
      }
      if (part === "CommandOrControl" || part === "CmdOrControl") {
        return isMac ? "⌘" : "Ctrl";
      }
      if (part.startsWith("Key") && part.length === 4) return part.slice(3);
      if (part.startsWith("Digit") && part.length === 6) return part.slice(5);
      if (part.startsWith("Numpad") && part.length === 7) {
        return `Num${part.slice(6)}`;
      }
      return KEY_DISPLAY_MAP[part] ?? part;
    })
    .join(" + ");
}

function getSystemColorScheme(): "dark" | "light" {
  return typeof window !== "undefined" &&
    window.matchMedia("(prefers-color-scheme: dark)").matches
    ? "dark"
    : "light";
}

function readStoredColorScheme(): "dark" | "light" | null {
  try {
    const val = localStorage.getItem(COLOR_SCHEME_STORAGE_KEY);
    return val === "dark" || val === "light" ? val : null;
  } catch {
    return null;
  }
}

function applyColorSchemeMeta(content: "dark" | "light" | "light dark") {
  const meta = document.querySelector<HTMLMetaElement>(
    'meta[name="color-scheme"]'
  );
  if (meta) {
    meta.content = content;
  }
  const nativeTheme =
    content === "dark" || content === "light" ? content : null;
  void invoke("set_window_theme", { theme: nativeTheme }).catch(() => {});
}

function ShizukaMark({
  active,
  muted,
}: {
  active: boolean;
  muted: boolean;
}) {
  return (
    <svg
      className={`brand-mark ${muted ? "muted" : active ? "active" : ""}`}
      viewBox="0 0 26 26"
      width="26"
      height="26"
      shapeRendering="geometricPrecision"
      aria-hidden="true"
    >
      <path
        className="mark-stroke"
        d="M 16.42 22.93 A 10.5 10.5 0 1 1 22.93 16.42"
        fill="none"
        strokeWidth="2.1"
        strokeLinecap="round"
      />
      <circle className="mark-led" cx="20.42" cy="20.42" r="2.15" />
      <rect className="mark-bars" x="8" y="10" width="2" height="6" rx="1" />
      <rect className="mark-bars" x="12" y="8" width="2" height="10" rx="1" />
      <rect className="mark-bars" x="16" y="10" width="2" height="6" rx="1" />
    </svg>
  );
}

function dbToRatio(db: number): number {
  const clamped = Math.max(-60, Math.min(0, db));
  return (clamped + 60) / 60;
}

function sliderCssVars(
  value: number,
  min: number,
  max: number
): CSSProperties {
  return {
    "--attr-min": String(min),
    "--attr-max": String(max),
    "--control-value": String(value),
  } as CSSProperties;
}

export function App() {
  const uid = useId();
  const ncSwitchId = `${uid}-nc-switch`;
  const micSelectId = `${uid}-mic-select`;
  const virtSelectId = `${uid}-virt-select`;
  const monSelectId = `${uid}-mon-select`;
  const modelStdId = `${uid}-model-std`;
  const modelLlId = `${uid}-model-ll`;
  const presetNatId = `${uid}-preset-nat`;
  const presetBalId = `${uid}-preset-bal`;
  const presetMaxId = `${uid}-preset-max`;
  const attenSliderId = `${uid}-atten-slider`;
  const gainSliderId = `${uid}-gain-slider`;
  const hpfSwitchId = `${uid}-hpf-switch`;
  const hpfHintId = `${uid}-hpf-hint`;
  const agcSwitchId = `${uid}-agc-switch`;
  const agcHintId = `${uid}-agc-hint`;
  const vadSwitchId = `${uid}-vad-switch`;
  const vadSliderId = `${uid}-vad-slider`;
  const vadHintId = `${uid}-vad-hint`;
  const inputMeterId = `${uid}-in-meter`;
  const outputMeterId = `${uid}-out-meter`;
  const monMeterId = `${uid}-mon-meter`;
  const monCleanId = `${uid}-mon-clean`;
  const monRawId = `${uid}-mon-raw`;
  const monVolSliderId = `${uid}-mon-vol-slider`;
  const settingsTitleId = `${uid}-settings-title`;
  const autostartSwitchId = `${uid}-autostart-switch`;
  const autostartHintId = `${uid}-autostart-hint`;
  const cueSoundSwitchId = `${uid}-cue-sound-switch`;
  const cueSoundHintId = `${uid}-cue-sound-hint`;

  const settingsDialogRef = useRef<HTMLDialogElement | null>(null);
  const lastReconnectCountRef = useRef<number>(0);
  const lastConfigRevisionRef = useRef<number>(0);
  const recordingActionRef = useRef<ShortcutAction | null>(null);

  const [devices, setDevices] = useState<DeviceListResponse | null>(null);
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [telemetry, setTelemetry] = useState<AudioTelemetry | null>(null);
  const [monitorClean, setMonitorClean] = useState(true);
  const [autostartEnabled, setAutostartEnabled] = useState(false);
  const [autostartBusy, setAutostartBusy] = useState(false);
  const [autostartError, setAutostartError] = useState<string | null>(null);
  const [recordingAction, setRecordingAction] =
    useState<ShortcutAction | null>(null);
  const [shortcutError, setShortcutError] = useState<string | null>(null);
  const [diagCopied, setDiagCopied] = useState(false);
  const [installingDriver, setInstallingDriver] = useState(false);
  const [installMessage, setInstallMessage] = useState<{
    ok: boolean;
    text: string;
  } | null>(null);

  recordingActionRef.current = recordingAction;

  const refreshDevicesAndConfig = useCallback(async () => {
    try {
      const [devs, cfg, autostart] = await Promise.all([
        invoke<DeviceListResponse>("get_devices"),
        invoke<AppConfig>("get_config"),
        invoke<boolean>("get_autostart_enabled").catch(() => false),
      ]);
      setDevices(devs);
      setConfig(cfg);
      setAutostartEnabled(autostart);
    } catch (err) {
      console.error("Failed to load audio devices/config:", err);
    }
  }, []);

  const stopShortcutRecording = useCallback(async () => {
    if (recordingActionRef.current === null) return;
    recordingActionRef.current = null;
    setRecordingAction(null);
    await invoke("set_shortcuts_paused", { paused: false }).catch(() => {});
  }, []);

  const startShortcutRecording = useCallback(async (action: ShortcutAction) => {
    setShortcutError(null);
    if (recordingActionRef.current === action) {
      await stopShortcutRecording();
      return;
    }
    try {
      await invoke("set_shortcuts_paused", { paused: true });
      recordingActionRef.current = action;
      setRecordingAction(action);
    } catch (err) {
      setShortcutError(typeof err === "string" ? err : String(err));
    }
  }, [stopShortcutRecording]);

  const closeSettingsDialog = useCallback(async () => {
    await stopShortcutRecording();
    const dialog = settingsDialogRef.current;
    if (!dialog || !dialog.open) return;
    const supportsNativeTopLayerExit =
      typeof window !== "undefined" &&
      Boolean(window.CSS?.supports?.("overlay", "auto"));
    if (!supportsNativeTopLayerExit) {
      dialog.setAttribute("data-closing", "");
      const animations = dialog.getAnimations({ subtree: true });
      if (animations.length > 0) {
        await Promise.race([
          Promise.allSettled(animations.map((a) => a.finished)),
          new Promise((r) => setTimeout(r, 400)),
        ]);
      }
      if (!dialog.hasAttribute("data-closing")) return;
      dialog.removeAttribute("data-closing");
    }
    dialog.close();
  }, [stopShortcutRecording]);

  const openSettingsDialog = useCallback(() => {
    const dialog = settingsDialogRef.current;
    if (!dialog || dialog.open) return;
    dialog.removeAttribute("data-closing");
    setAutostartError(null);
    setShortcutError(null);
    setDiagCopied(false);
    void invoke<boolean>("get_autostart_enabled")
      .then(setAutostartEnabled)
      .catch(() => {});
    void invoke<string | null>("get_shortcut_warning")
      .then((warn) => {
        if (warn) setShortcutError(warn);
      })
      .catch(() => {});
    dialog.showModal();
  }, []);

  // Capture keyboard shortcut while recording inside Settings dialog
  useEffect(() => {
    if (recordingAction === null) return;

    const onKeyDown = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();

      if (e.code === "Escape" || e.key === "Escape") {
        setShortcutError(null);
        void stopShortcutRecording();
        return;
      }

      const parsed = buildAcceleratorFromKeyEvent(e);
      if (parsed === null) {
        return;
      }
      if (!parsed.ok) {
        setShortcutError(parsed.error);
        return;
      }

      const targetAction = recordingAction;
      void (async () => {
        try {
          // Backend validates OS registration, commits, and atomically unpauses shortcuts
          const updated = await invoke<AppConfig>("set_shortcut", {
            action: targetAction,
            accelerator: parsed.value,
          });
          setConfig(updated);
          recordingActionRef.current = null;
          setRecordingAction(null);
          setShortcutError(null);
        } catch (err) {
          setShortcutError(typeof err === "string" ? err : String(err));
          recordingActionRef.current = null;
          setRecordingAction(null);
          await invoke("set_shortcuts_paused", { paused: false }).catch(
            () => {}
          );
        }
      })();
    };

    window.addEventListener("keydown", onKeyDown, true);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
    };
  }, [recordingAction, stopShortcutRecording]);

  useEffect(() => {
    const dialog = settingsDialogRef.current;
    if (!dialog) return;

    const onClose = () => {
      dialog.removeAttribute("data-closing");
      dialog.returnValue = "";
      if (recordingActionRef.current !== null) {
        recordingActionRef.current = null;
        setRecordingAction(null);
        void invoke("set_shortcuts_paused", { paused: false }).catch(() => {});
      }
    };
    dialog.addEventListener("close", onClose);

    const supportsNativeTopLayerExit =
      typeof window !== "undefined" &&
      Boolean(window.CSS?.supports?.("overlay", "auto"));
    const onCancel = (e: Event) => {
      // Protect both Chromium and WKWebView from closing dialog when Esc cancels key recording
      if (recordingActionRef.current !== null) {
        e.preventDefault();
        setShortcutError(null);
        void stopShortcutRecording();
        return;
      }
      if (!supportsNativeTopLayerExit) {
        e.preventDefault();
        void closeSettingsDialog();
      }
    };
    dialog.addEventListener("cancel", onCancel);

    // Fallback for browsers without <dialog closedby="any"> (WKWebView / Safari):
    // require both pointerdown and pointerup outside the dialog bounding box.
    const supportsClosedBy =
      typeof HTMLDialogElement !== "undefined" &&
      "closedBy" in HTMLDialogElement.prototype;
    let pressedOutside = false;
    const isOutside = (e: PointerEvent) => {
      const r = dialog.getBoundingClientRect();
      return (
        e.clientX < r.left ||
        e.clientX > r.right ||
        e.clientY < r.top ||
        e.clientY > r.bottom
      );
    };
    const onPointerDown = (e: PointerEvent) => {
      pressedOutside = isOutside(e);
    };
    const onPointerUp = (e: PointerEvent) => {
      if (
        recordingActionRef.current === null &&
        pressedOutside &&
        isOutside(e)
      ) {
        void closeSettingsDialog();
      }
      pressedOutside = false;
    };
    if (!supportsClosedBy) {
      dialog.addEventListener("pointerdown", onPointerDown);
      dialog.addEventListener("pointerup", onPointerUp);
    }

    return () => {
      dialog.removeEventListener("close", onClose);
      dialog.removeEventListener("cancel", onCancel);
      if (!supportsClosedBy) {
        dialog.removeEventListener("pointerdown", onPointerDown);
        dialog.removeEventListener("pointerup", onPointerUp);
      }
    };
  }, [closeSettingsDialog, stopShortcutRecording]);

  useEffect(() => {
    const initialStored = readStoredColorScheme();
    applyColorSchemeMeta(initialStored ?? "light dark");

    const onStorage = (e: StorageEvent) => {
      if (e.key === COLOR_SCHEME_STORAGE_KEY || e.key === null) {
        const next = readStoredColorScheme();
        applyColorSchemeMeta(next ?? "light dark");
      }
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);

  useEffect(() => {
    void refreshDevicesAndConfig();
    const timer = setInterval(async () => {
      try {
        const snap = await invoke<AudioTelemetry>("get_telemetry");
        setTelemetry(snap);
        setConfig((prev) =>
          prev && prev.nc_enabled !== snap.nc_enabled
            ? { ...prev, nc_enabled: snap.nc_enabled }
            : prev
        );
        if (snap.status.reconnect_count !== lastReconnectCountRef.current) {
          lastReconnectCountRef.current = snap.status.reconnect_count;
          void refreshDevicesAndConfig();
        } else if (snap.config_revision !== lastConfigRevisionRef.current) {
          lastConfigRevisionRef.current = snap.config_revision;
          void invoke<AppConfig>("get_config")
            .then(setConfig)
            .catch(() => {});
        }
      } catch {
        // Ignore transient IPC error during shutdown
      }
    }, 50);
    return () => clearInterval(timer);
  }, [refreshDevicesAndConfig]);

  const ncEnabled = telemetry?.nc_enabled ?? config?.nc_enabled ?? true;
  const micMuted = telemetry?.mic_muted ?? false;

  const handleToggleNc = async (next: boolean) => {
    setConfig((prev) => (prev ? { ...prev, nc_enabled: next } : prev));
    await invoke("set_nc_enabled", { enabled: next });
  };

  const handleToggleMute = async () => {
    const next = !micMuted;
    await invoke("set_mic_muted", { muted: next });
  };

  // Two-state color-scheme toggle: pins explicit opposite of rendered scheme,
  // or removes stored override when returning to match the current OS scheme.
  const handleToggleColorScheme = () => {
    const sys = getSystemColorScheme();
    const stored = readStoredColorScheme();
    const rendered = stored ?? sys;
    const target: "dark" | "light" = rendered === "dark" ? "light" : "dark";

    if (target !== sys) {
      try {
        localStorage.setItem(COLOR_SCHEME_STORAGE_KEY, target);
      } catch {
        // Still apply to current session even if storage is unavailable
      }
      applyColorSchemeMeta(target);
    } else {
      try {
        localStorage.removeItem(COLOR_SCHEME_STORAGE_KEY);
      } catch {
        // Ignore storage error
      }
      applyColorSchemeMeta("light dark");
    }
  };

  const handleDeviceChange = async (
    nextIn: string | null,
    nextOut: string | null,
    nextMon: string | null
  ) => {
    if (!config) return;
    const optimistic: AppConfig = {
      ...config,
      input_device: nextIn,
      output_device: nextOut,
      monitor_device: nextMon,
    };
    setConfig(optimistic);
    const synced = await invoke<AppConfig>("select_devices", {
      inputDevice: nextIn,
      outputDevice: nextOut,
      monitorDevice: nextMon,
    });
    setConfig(synced);
  };

  const handleModelVariantChange = async (nextVariant: ModelVariant) => {
    setConfig((prev) =>
      prev ? { ...prev, model_variant: nextVariant } : prev
    );
    await invoke("set_model_variant", { modelVariant: nextVariant });
  };

  const handleParamChange = async (
    attenLimDb: number,
    postFilterBeta: number,
    inputGainDb: number,
    vadGateEnabled: boolean,
    vadThresholdDb: number,
    hpfEnabled: boolean,
    agcEnabled: boolean
  ) => {
    if (!config) return;
    const minThresh = config.min_db_thresh;
    setConfig({
      ...config,
      atten_lim_db: attenLimDb,
      post_filter_beta: postFilterBeta,
      input_gain_db: inputGainDb,
      vad_gate_enabled: vadGateEnabled,
      vad_threshold_db: vadThresholdDb,
      hpf_enabled: hpfEnabled,
      agc_enabled: agcEnabled,
    });
    await invoke("update_dsp_params", {
      attenLimDb,
      postFilterBeta,
      inputGainDb,
      minDbThresh: minThresh,
      vadGateEnabled,
      vadThresholdDb,
      hpfEnabled,
      agcEnabled,
    });
  };

  const handleClearShortcut = async (action: ShortcutAction) => {
    setShortcutError(null);
    await stopShortcutRecording();
    try {
      const updated = await invoke<AppConfig>("set_shortcut", {
        action,
        accelerator: null,
      });
      setConfig(updated);
    } catch (err) {
      setShortcutError(typeof err === "string" ? err : String(err));
    }
  };

  const handleToggleShortcutSound = async (enabled: boolean) => {
    setConfig((prev) =>
      prev ? { ...prev, shortcut_sound_enabled: enabled } : prev
    );
    await invoke("set_shortcut_sound_enabled", { enabled });
  };

  const handleToggleLiveMonitor = async () => {
    const cur = telemetry?.live_monitor_enabled ?? false;
    await invoke("set_live_monitor", { enabled: !cur });
  };

  const handleMonitorModeChange = async (clean: boolean) => {
    setMonitorClean(clean);
    await invoke("set_monitor_mode", { clean });
  };

  const handleMonitorVolumeChange = async (pct: number) => {
    const clampedPct = Math.max(0, Math.min(100, pct));
    const vol = clampedPct / 100;
    setConfig((prev) => (prev ? { ...prev, monitor_volume: vol } : prev));
    await invoke("set_monitor_volume", { volume: vol });
  };

  const handleQuit = async () => {
    await invoke("quit_app");
  };

  const handleToggleAutostart = async (next: boolean) => {
    setAutostartBusy(true);
    setAutostartError(null);
    try {
      const actual = await invoke<boolean>("set_autostart_enabled", {
        enabled: next,
      });
      setAutostartEnabled(actual);
    } catch (err) {
      setAutostartError(typeof err === "string" ? err : String(err));
    } finally {
      setAutostartBusy(false);
    }
  };

  const handleCopyDiagnostics = async () => {
    try {
      const text = await invoke<string>("get_diagnostics_text");
      await navigator.clipboard.writeText(text);
      setDiagCopied(true);
      window.setTimeout(() => setDiagCopied(false), 2200);
    } catch (err) {
      console.error("Failed to copy diagnostics:", err);
    }
  };

  const handleInstallVirtualDriver = async () => {
    setInstallingDriver(true);
    setInstallMessage(null);
    try {
      const res = await invoke<InstallDriverResult>("install_virtual_driver");
      setInstallMessage({ ok: res.success, text: res.message });
      await refreshDevicesAndConfig();
    } catch (err) {
      setInstallMessage({
        ok: false,
        text: typeof err === "string" ? err : String(err),
      });
    } finally {
      setInstallingDriver(false);
    }
  };

  const modelVariant: ModelVariant =
    config?.model_variant ?? telemetry?.model_variant ?? "standard";
  const attenDb = config?.atten_lim_db ?? 65;
  const pfBeta = config?.post_filter_beta ?? 0.0;
  const gainDb = config?.input_gain_db ?? 0;
  const vadGateEnabled =
    config?.vad_gate_enabled ?? telemetry?.vad_gate_enabled ?? true;
  const vadThreshDb = config?.vad_threshold_db ?? -5;
  const hpfEnabled = config?.hpf_enabled ?? telemetry?.hpf_enabled ?? true;
  const agcEnabled = config?.agc_enabled ?? telemetry?.agc_enabled ?? false;
  const agcGainDb = telemetry?.agc_gain_db ?? 0;
  const clippingActive = telemetry?.clipping_active ?? false;
  const vadSpeaking = telemetry?.vad_speaking ?? false;
  const liveMonitorEnabled = telemetry?.live_monitor_enabled ?? false;
  const monVolPct = Math.round(
    (config?.monitor_volume ?? telemetry?.monitor_volume ?? 0.8) * 100
  );
  const monLevelRatio = liveMonitorEnabled
    ? dbToRatio(telemetry?.monitor_rms_db ?? -90)
    : 0;
  const isMac = devices?.platform === "macos";
  const shortcutSoundEnabled = config?.shortcut_sound_enabled ?? true;
  const connState = telemetry?.status.connection_state ?? "running";
  const connStateLabel =
    connState === "running"
      ? "稼働中"
      : connState === "waiting_device"
      ? "再接続待ち"
      : connState === "reconnecting"
      ? "再接続中"
      : "停止中";

  const activePreset =
    attenDb <= 30 ? "natural" : attenDb >= 90 ? "max" : "balanced";
  const attenValueText =
    attenDb >= 100 ? "無制限 (100 dB)" : `${attenDb.toFixed(0)} dB`;
  const gainValueText =
    gainDb > 0 ? `+${gainDb.toFixed(1)} dB` : `${gainDb.toFixed(1)} dB`;
  const vadValueText =
    vadThreshDb > 0
      ? `+${vadThreshDb.toFixed(1)} dB`
      : `${vadThreshDb.toFixed(1)} dB`;
  const agcLiveText =
    agcGainDb >= 0
      ? `+${agcGainDb.toFixed(1)} dB`
      : `${agcGainDb.toFixed(1)} dB`;

  return (
    <div className="app-shell">
      <header className="topbar">
        <div className="brand">
          <ShizukaMark
            active={Boolean(telemetry?.stream_running && ncEnabled)}
            muted={micMuted}
          />
          <h1 className="brand-title">Shizuka</h1>
        </div>
        <div className="badge-row">
          <span className="stat-pill">
            CPU {(telemetry?.cpu_load_pct ?? 0).toFixed(1)}%
          </span>
          <span className="stat-pill">
            {(telemetry?.latency_ms ?? 10).toFixed(0)} ms
          </span>
          <button
            type="button"
            className="icon-btn"
            onClick={handleToggleColorScheme}
          >
            <span className="theme-branch if-dark">
              <Sun size={14} aria-hidden="true" />
              <span className="visually-hidden">ライトモードに切り替え</span>
            </span>
            <span className="theme-branch if-light">
              <Moon size={14} aria-hidden="true" />
              <span className="visually-hidden">ダークモードに切り替え</span>
            </span>
          </button>
          <button
            type="button"
            className="icon-btn"
            onClick={() => void refreshDevicesAndConfig()}
            aria-label="オーディオデバイスを再スキャン"
          >
            <RefreshCw size={14} aria-hidden="true" />
          </button>
          <button
            type="button"
            className="icon-btn"
            onClick={openSettingsDialog}
            aria-label="アプリ設定を開く"
          >
            <Settings size={14} aria-hidden="true" />
          </button>
        </div>
      </header>

      <main className="main-stack">
        {connState === "waiting_device" && (
          <div className="status-banner" role="status">
            <span className="status-banner-dot" aria-hidden="true" />
            <span>
              <strong>マイク再接続待ち：</strong>
              {telemetry?.status.waiting_device_name
                ? `「${telemetry.status.waiting_device_name}」が接続されると自動で再開します。`
                : "マイクが接続されると自動で再開します。"}
            </span>
          </div>
        )}

        {connState === "reconnecting" && (
          <div className="status-banner" role="status">
            <span className="status-banner-dot" aria-hidden="true" />
            <span>オーディオストリームを自動復旧しています...</span>
          </div>
        )}

        {telemetry?.status.last_error && (
          <div className="error-banner" role="alert">
            <TriangleAlert
              size={13}
              aria-hidden="true"
              style={{
                verticalAlign: "text-bottom",
                marginInlineEnd: "0.3rem",
              }}
            />
            {telemetry.status.last_error}
          </div>
        )}

        {/* Primary Noise Cancellation Switch, Mic Mute & Live Meters */}
        <section
          className={`card hero-card ${micMuted ? "is-muted" : ""}`}
          aria-labelledby={`${uid}-hero-heading`}
        >
          <div className="hero-top">
            <div className="hero-title-row">
              <span className="hero-icon-box" aria-hidden="true">
                <AudioLines size={15} />
              </span>
              <label
                id={`${uid}-hero-heading`}
                htmlFor={ncSwitchId}
                className="hero-title"
              >
                AI ノイズキャンセリング
              </label>
              <span
                className={`nc-state-pill ${ncEnabled ? "on" : "off"}`}
                aria-hidden="true"
              >
                <span className="nc-state-dot" />
                <span>{ncEnabled ? "ON" : "スルー"}</span>
              </span>
            </div>
            <input
              id={ncSwitchId}
              name="nc_enabled"
              type="checkbox"
              role={supportsNativeSwitch ? undefined : "switch"}
              className="nc-toggle"
              checked={ncEnabled}
              onChange={(e) => void handleToggleNc(e.currentTarget.checked)}
              {...({ switch: "" } as Record<string, string>)}
            />
          </div>

          <div className="hero-mute-row">
            <div className="hero-mute-info">
              <span
                className={`mute-status-dot ${micMuted ? "is-muted" : ""}`}
                aria-hidden="true"
              />
              <span className="hero-mute-label">
                {micMuted ? "マイクミュート中（無音）" : "マイク送信中"}
              </span>
              {config?.shortcut_toggle_mute && (
                <kbd className="inline-kbd">
                  {formatShortcutLabel(config.shortcut_toggle_mute, isMac)}
                </kbd>
              )}
            </div>
            <button
              type="button"
              className={`mute-pill-btn ${micMuted ? "is-muted" : ""}`}
              aria-pressed={micMuted}
              onClick={() => void handleToggleMute()}
            >
              {micMuted ? (
                <MicOff size={12} aria-hidden="true" />
              ) : (
                <Mic size={12} aria-hidden="true" />
              )}
              <span>{micMuted ? "ミュート解除" : "ミュート"}</span>
            </button>
          </div>

          <div className="meters-box">
            <div className="meter-row">
              <div className="meter-label">
                <label htmlFor={inputMeterId}>入力マイク (処理前)</label>
                <span>{(telemetry?.input_rms_db ?? -90).toFixed(0)} dB</span>
              </div>
              <progress
                id={inputMeterId}
                className="audio-meter raw"
                max={1}
                value={dbToRatio(telemetry?.input_rms_db ?? -90)}
              />
            </div>

            <div className="meter-row">
              <div className="meter-label">
                <span className="meter-label-left">
                  <label htmlFor={outputMeterId}>
                    {micMuted
                      ? "ミュート中 (無音送信)"
                      : ncEnabled
                      ? "ノイズ除去後"
                      : "スルー出力 (未処理)"}
                  </label>
                  {!micMuted &&
                    ncEnabled &&
                    (telemetry?.noise_reduced_db ?? 0) > 1.5 && (
                      <span className="reduction-badge">
                        -{(telemetry?.noise_reduced_db ?? 0).toFixed(0)} dB
                      </span>
                    )}
                  {clippingActive && (
                    <span
                      className="clip-badge"
                      role="status"
                      title="入力または出力がリミッターに達しました。入力ゲインを下げてください。"
                    >
                      CLIP
                    </span>
                  )}
                </span>
                <span>
                  {micMuted
                    ? "-∞ dB"
                    : `${(telemetry?.output_rms_db ?? -90).toFixed(0)} dB`}
                </span>
              </div>
              <progress
                id={outputMeterId}
                className={`audio-meter ${
                  micMuted ? "muted" : ncEnabled ? "" : "raw"
                }`}
                max={1}
                value={
                  micMuted ? 0 : dbToRatio(telemetry?.output_rms_db ?? -90)
                }
              />
            </div>
          </div>
        </section>

        {/* Audio Device Routing */}
        <section className="card" aria-labelledby={`${uid}-routing-heading`}>
          <div className="section-header">
            <h2 id={`${uid}-routing-heading`} className="section-title">
              <Waves size={14} aria-hidden="true" />
              <span>入出力デバイス設定</span>
            </h2>
            <span
              className={`status-indicator ${
                telemetry?.virtual_out_connected ? "connected" : ""
              }`}
            >
              <span className="status-dot" aria-hidden="true" />
              <span>
                {telemetry?.virtual_out_connected
                  ? "仮想マイク出力中"
                  : "単体テストモード"}
              </span>
            </span>
          </div>

          <div className="field">
            <label htmlFor={micSelectId} className="field-label">
              使用する物理マイク
            </label>
            <select
              id={micSelectId}
              name="input_device"
              className="brand-select"
              value={config?.input_device ?? ""}
              onChange={(e) =>
                void handleDeviceChange(
                  e.target.value || null,
                  config?.output_device ?? null,
                  config?.monitor_device ?? null
                )
              }
            >
              <option value="">システム既定のマイク (自動選択)</option>
              {devices?.inputs.map((d) => (
                <option key={d.name} value={d.name}>
                  {d.is_default ? "★ " : ""}
                  {d.name} ({Math.round(d.sample_rate / 1000)}kHz)
                </option>
              ))}
            </select>
          </div>

          <div className="field">
            <label htmlFor={virtSelectId} className="field-label">
              仮想マイク出力先 (会議アプリ連携)
            </label>
            <select
              id={virtSelectId}
              name="output_device"
              className="brand-select"
              value={config?.output_device ?? ""}
              onChange={(e) =>
                void handleDeviceChange(
                  config?.input_device ?? null,
                  e.target.value || null,
                  config?.monitor_device ?? null
                )
              }
            >
              <option value="">出力しない (録音テストのみ)</option>
              {devices?.outputs.map((d) => (
                <option key={d.name} value={d.name}>
                  {d.is_virtual_cable ? "★推奨 [仮想] " : ""}
                  {d.name}
                </option>
              ))}
            </select>
          </div>

          {devices && !devices.has_virtual_cable && (
            <div className="notice-banner">
              <p>
                <TriangleAlert
                  size={13}
                  aria-hidden="true"
                  style={{
                    verticalAlign: "text-bottom",
                    marginInlineEnd: "0.3rem",
                  }}
                />
                <strong>仮想マイクが未導入です（会議アプリ連携用）：</strong>
                <br />
                {isMac ? (
                  <>
                    同梱の MIT ライセンス仮想マイク（
                    <code>Shizuka Virtual Mic</code>
                    ）をワンクリックで導入できます（再起動不要）。
                  </>
                ) : (
                  <>
                    VB-Audio 公式の WHQL 署名済み仮想ケーブル（Donationware:
                    vb-audio.com）を自動取得・サイレント導入します。導入後は会議アプリのマイクに{" "}
                    <code>CABLE Output</code> を選択してください。
                  </>
                )}
              </p>
              <button
                type="button"
                className="btn primary"
                disabled={installingDriver}
                onClick={() => void handleInstallVirtualDriver()}
              >
                {installingDriver ? (
                  <progress
                    className="loading-spinner"
                    aria-label="ドライバをセットアップ中"
                  />
                ) : (
                  <Download size={14} aria-hidden="true" />
                )}
                <span>
                  {installingDriver
                    ? "公式ドライバを取得・セットアップ中..."
                    : isMac
                    ? "Shizuka Virtual Mic (MIT) を導入"
                    : "仮想マイクドライバを自動インストール"}
                </span>
              </button>
            </div>
          )}

          {installMessage && (
            <div
              className={installMessage.ok ? "success-banner" : "error-banner"}
              role={installMessage.ok ? "status" : "alert"}
            >
              {installMessage.ok ? (
                <CheckCircle2
                  size={13}
                  aria-hidden="true"
                  style={{
                    verticalAlign: "text-bottom",
                    marginInlineEnd: "0.3rem",
                  }}
                />
              ) : (
                <TriangleAlert
                  size={13}
                  aria-hidden="true"
                  style={{
                    verticalAlign: "text-bottom",
                    marginInlineEnd: "0.3rem",
                  }}
                />
              )}
              {installMessage.text}
            </div>
          )}
        </section>

        {/* Secondary Disclosure: Noise Suppression Strength, AI Model, HPF, AGC & VAD Gate */}
        <details className="disclosure-card" open>
          <summary>
            <Sliders size={14} aria-hidden="true" />
            <span>除去強度・音質チューニング</span>
            <span className="summary-meta">
              {modelVariant === "standard" ? "DFN3" : "DFN3-LL"} ·{" "}
              {attenValueText}
            </span>
          </summary>
          <div className="disclosure-body">
            <fieldset className="segmented-fieldset">
              <legend className="fieldset-legend">AI モデル選択</legend>
              <div className="segmented-grid">
                <label
                  htmlFor={modelStdId}
                  className={`seg-option ${
                    modelVariant === "standard" ? "is-selected" : ""
                  }`}
                >
                  <input
                    id={modelStdId}
                    type="radio"
                    name="model_variant"
                    className="visually-hidden"
                    checked={modelVariant === "standard"}
                    onChange={() => void handleModelVariantChange("standard")}
                  />
                  <span>高精度 DFN3 (推奨)</span>
                </label>
                <label
                  htmlFor={modelLlId}
                  className={`seg-option ${
                    modelVariant === "low_latency" ? "is-selected" : ""
                  }`}
                >
                  <input
                    id={modelLlId}
                    type="radio"
                    name="model_variant"
                    className="visually-hidden"
                    checked={modelVariant === "low_latency"}
                    onChange={() =>
                      void handleModelVariantChange("low_latency")
                    }
                  />
                  <span>超低遅延 DFN3-LL</span>
                </label>
              </div>
            </fieldset>

            <fieldset className="segmented-fieldset">
              <legend className="fieldset-legend">ノイズ除去プリセット</legend>
              <div className="segmented-grid">
                <label
                  htmlFor={presetNatId}
                  className={`seg-option ${
                    activePreset === "natural" ? "is-selected" : ""
                  }`}
                >
                  <input
                    id={presetNatId}
                    type="radio"
                    name="nc_preset"
                    className="visually-hidden"
                    checked={activePreset === "natural"}
                    onChange={() =>
                      void handleParamChange(
                        24,
                        0.0,
                        gainDb,
                        vadGateEnabled,
                        vadThreshDb,
                        hpfEnabled,
                        agcEnabled
                      )
                    }
                  />
                  <span>ナチュラル</span>
                </label>
                <label
                  htmlFor={presetBalId}
                  className={`seg-option ${
                    activePreset === "balanced" ? "is-selected" : ""
                  }`}
                >
                  <input
                    id={presetBalId}
                    type="radio"
                    name="nc_preset"
                    className="visually-hidden"
                    checked={activePreset === "balanced"}
                    onChange={() =>
                      void handleParamChange(
                        65,
                        0.0,
                        gainDb,
                        vadGateEnabled,
                        vadThreshDb,
                        hpfEnabled,
                        agcEnabled
                      )
                    }
                  />
                  <span>バランス (推奨)</span>
                </label>
                <label
                  htmlFor={presetMaxId}
                  className={`seg-option ${
                    activePreset === "max" ? "is-selected" : ""
                  }`}
                >
                  <input
                    id={presetMaxId}
                    type="radio"
                    name="nc_preset"
                    className="visually-hidden"
                    checked={activePreset === "max"}
                    onChange={() =>
                      void handleParamChange(
                        100,
                        0.015,
                        gainDb,
                        vadGateEnabled,
                        vadThreshDb,
                        hpfEnabled,
                        agcEnabled
                      )
                    }
                  />
                  <span>最強 (打鍵音)</span>
                </label>
              </div>
            </fieldset>

            <div className="slider-row">
              <div className="slider-top">
                <label htmlFor={attenSliderId}>最大ノイズ減衰量</label>
                <span className="slider-val">{attenValueText}</span>
              </div>
              <input
                id={attenSliderId}
                name="atten_lim_db"
                type="range"
                className="range-control"
                min={10}
                max={100}
                step={1}
                value={attenDb}
                aria-valuetext={attenValueText}
                style={sliderCssVars(attenDb, 10, 100)}
                onChange={(e) =>
                  void handleParamChange(
                    Number(e.target.value),
                    pfBeta,
                    gainDb,
                    vadGateEnabled,
                    vadThreshDb,
                    hpfEnabled,
                    agcEnabled
                  )
                }
              />
            </div>

            <div className="slider-row">
              <div className="slider-top">
                <label htmlFor={gainSliderId}>マイク入力ゲイン補正</label>
                <span className="slider-val">{gainValueText}</span>
              </div>
              <input
                id={gainSliderId}
                name="input_gain_db"
                type="range"
                className="range-control"
                min={-12}
                max={12}
                step={0.5}
                value={gainDb}
                aria-valuetext={gainValueText}
                style={sliderCssVars(gainDb, -12, 12)}
                onChange={(e) =>
                  void handleParamChange(
                    attenDb,
                    pfBeta,
                    Number(e.target.value),
                    vadGateEnabled,
                    vadThreshDb,
                    hpfEnabled,
                    agcEnabled
                  )
                }
              />
            </div>

            {/* Voice Conditioning Toggles: HPF 80Hz & VAD-Gated AGC */}
            <div className="dsp-toggle-section">
              <div className="dsp-toggle-row">
                <div className="dsp-toggle-copy">
                  <label htmlFor={hpfSwitchId} className="vad-title">
                    ハイパスフィルタ (HPF 65Hz)
                  </label>
                  <span id={hpfHintId} className="field-hint">
                    机の振動・アーム衝撃・吹かれ音などの超低域ノイズをカット
                  </span>
                </div>
                <input
                  id={hpfSwitchId}
                  name="hpf_enabled"
                  type="checkbox"
                  role={supportsNativeSwitch ? undefined : "switch"}
                  className="nc-toggle compact"
                  checked={hpfEnabled}
                  aria-describedby={hpfHintId}
                  onChange={(e) =>
                    void handleParamChange(
                      attenDb,
                      pfBeta,
                      gainDb,
                      vadGateEnabled,
                      vadThreshDb,
                      e.currentTarget.checked,
                      agcEnabled
                    )
                  }
                  {...({ switch: "" } as Record<string, string>)}
                />
              </div>

              <div className="dsp-toggle-row">
                <div className="dsp-toggle-copy">
                  <div className="vad-title-group">
                    <label htmlFor={agcSwitchId} className="vad-title">
                      オートゲイン (AGC)
                    </label>
                    {agcEnabled && (
                      <span className="agc-live-pill" aria-hidden="true">
                        {agcLiveText}
                      </span>
                    )}
                  </div>
                  <span id={agcHintId} className="field-hint">
                    発話区間のみで声の音量を自動均一化（無音時のノイズ増幅を防止）
                  </span>
                </div>
                <input
                  id={agcSwitchId}
                  name="agc_enabled"
                  type="checkbox"
                  role={supportsNativeSwitch ? undefined : "switch"}
                  className="nc-toggle compact"
                  checked={agcEnabled}
                  aria-describedby={agcHintId}
                  onChange={(e) =>
                    void handleParamChange(
                      attenDb,
                      pfBeta,
                      gainDb,
                      vadGateEnabled,
                      vadThreshDb,
                      hpfEnabled,
                      e.currentTarget.checked
                    )
                  }
                  {...({ switch: "" } as Record<string, string>)}
                />
              </div>
            </div>

            <div className="vad-section">
              <div className="vad-header">
                <div className="vad-title-group">
                  <label htmlFor={vadSwitchId} className="vad-title">
                    無音時ノイズゲート (VAD)
                  </label>
                  <span
                    className={`vad-live-pill ${
                      vadGateEnabled && ncEnabled && vadSpeaking
                        ? "speaking"
                        : ""
                    }`}
                    aria-hidden="true"
                  >
                    <span className="vad-live-dot" />
                    <span>
                      {!vadGateEnabled
                        ? "OFF"
                        : !ncEnabled
                        ? "待機"
                        : vadSpeaking
                        ? "発話中"
                        : "無音遮断"}
                    </span>
                  </span>
                </div>
                <input
                  id={vadSwitchId}
                  name="vad_gate_enabled"
                  type="checkbox"
                  role={supportsNativeSwitch ? undefined : "switch"}
                  className="nc-toggle compact"
                  checked={vadGateEnabled}
                  onChange={(e) =>
                    void handleParamChange(
                      attenDb,
                      pfBeta,
                      gainDb,
                      e.currentTarget.checked,
                      vadThreshDb,
                      hpfEnabled,
                      agcEnabled
                    )
                  }
                  {...({ switch: "" } as Record<string, string>)}
                />
              </div>

              <div className="slider-row">
                <div className="slider-top">
                  <label htmlFor={vadSliderId}>
                    VAD ゲート感度 (声の判定閾値)
                  </label>
                  <span className="slider-val">{vadValueText}</span>
                </div>
                <span id={vadHintId} className="field-hint">
                  高くするほど非発話時の打鍵音・環境音を厳しく完全ミュートします
                </span>
                <input
                  id={vadSliderId}
                  name="vad_threshold_db"
                  type="range"
                  className="range-control"
                  min={-15}
                  max={8}
                  step={0.5}
                  value={vadThreshDb}
                  disabled={!vadGateEnabled}
                  aria-valuetext={vadValueText}
                  aria-describedby={vadHintId}
                  style={sliderCssVars(vadThreshDb, -15, 8)}
                  onChange={(e) =>
                    void handleParamChange(
                      attenDb,
                      pfBeta,
                      gainDb,
                      vadGateEnabled,
                      Number(e.target.value),
                      hpfEnabled,
                      agcEnabled
                    )
                  }
                />
              </div>
            </div>
          </div>
        </details>

        {/* Secondary Disclosure: Discord-Style Real-Time Mic Test */}
        <details className="disclosure-card" open>
          <summary>
            <Headphones size={14} aria-hidden="true" />
            <span>マイクテスト</span>
            {liveMonitorEnabled && (
              <span className="test-live-pill" aria-hidden="true">
                <span className="test-live-dot" />
                <span>テスト中</span>
              </span>
            )}
          </summary>
          <div className="disclosure-body mic-test-body">
            {/* Row 1: Icon-only Transport Button + Discord-style Segmented Vertical Bar Meter */}
            <div className="mic-test-bar-row">
              <button
                type="button"
                className={`mic-test-btn ${
                  liveMonitorEnabled ? "is-active" : ""
                }`}
                aria-pressed={liveMonitorEnabled}
                aria-label={
                  liveMonitorEnabled ? "マイクテストを停止" : "マイクテストを開始"
                }
                onClick={() => void handleToggleLiveMonitor()}
              >
                {liveMonitorEnabled ? (
                  <Square size={13} fill="currentColor" aria-hidden="true" />
                ) : (
                  <Play size={14} fill="currentColor" aria-hidden="true" />
                )}
              </button>
              <label htmlFor={monMeterId} className="visually-hidden">
                マイクテスト音量レベル
              </label>
              <progress
                id={monMeterId}
                className="discord-meter"
                max={1}
                value={monLevelRatio}
              />
            </div>

            {/* Row 2: Instant Denoised / Raw Audio A/B Toggle */}
            <fieldset className="segmented-fieldset mic-test-mode">
              <legend className="visually-hidden">
                マイクテストの音声モード
              </legend>
              <div className="segmented-grid">
                <label
                  htmlFor={monCleanId}
                  className={`seg-option ${monitorClean ? "is-selected" : ""}`}
                >
                  <input
                    id={monCleanId}
                    type="radio"
                    name="mic_test_mode"
                    className="visually-hidden"
                    checked={monitorClean}
                    onChange={() => void handleMonitorModeChange(true)}
                  />
                  <span>ノイズ除去</span>
                </label>
                <label
                  htmlFor={monRawId}
                  className={`seg-option ${!monitorClean ? "is-selected" : ""}`}
                >
                  <input
                    id={monRawId}
                    type="radio"
                    name="mic_test_mode"
                    className="visually-hidden"
                    checked={!monitorClean}
                    onChange={() => void handleMonitorModeChange(false)}
                  />
                  <span>生音声</span>
                </label>
              </div>
            </fieldset>

            {/* Row 3: Compact Inline Output Volume Gauge */}
            <div className="inline-volume-row">
              <label htmlFor={monVolSliderId} className="inline-vol-icon">
                {monVolPct === 0 ? (
                  <VolumeX size={15} aria-hidden="true" />
                ) : (
                  <Volume2 size={15} aria-hidden="true" />
                )}
                <span className="visually-hidden">マイクテスト出力音量</span>
              </label>
              <input
                id={monVolSliderId}
                name="monitor_volume"
                type="range"
                className="range-control inline-range"
                min={0}
                max={100}
                step={1}
                value={monVolPct}
                aria-valuetext={`${monVolPct}%`}
                style={sliderCssVars(monVolPct, 0, 100)}
                onChange={(e) =>
                  void handleMonitorVolumeChange(Number(e.target.value))
                }
              />
              <span className="inline-vol-val" aria-hidden="true">
                {monVolPct}%
              </span>
            </div>

            {/* Row 4: Compact Speaker / Headphone Dropdown */}
            <div className="select-with-icon">
              <label htmlFor={monSelectId} className="visually-hidden">
                テスト再生用スピーカー / ヘッドホン
              </label>
              <Volume2
                size={14}
                className="select-leading-icon"
                aria-hidden="true"
              />
              <select
                id={monSelectId}
                name="monitor_device"
                className="brand-select has-leading-icon"
                value={config?.monitor_device ?? ""}
                onChange={(e) =>
                  void handleDeviceChange(
                    config?.input_device ?? null,
                    config?.output_device ?? null,
                    e.target.value || null
                  )
                }
              >
                <option value="">既定のスピーカー / ヘッドホン</option>
                {devices?.outputs
                  .filter((d) => !d.is_virtual_cable)
                  .map((d) => (
                    <option key={d.name} value={d.name}>
                      {d.name}
                    </option>
                  ))}
              </select>
            </div>
          </div>
        </details>
      </main>

      <footer className="footer-bar">
        <span>[×] でバックグラウンド動作</span>
        <button
          type="button"
          className="quit-btn"
          onClick={() => void handleQuit()}
        >
          アプリを終了
        </button>
      </footer>

      {/* Native Modal Settings Dialog (<dialog> with light-dismiss & top-layer transitions) */}
      <dialog
        ref={settingsDialogRef}
        className="settings-dialog"
        aria-labelledby={settingsTitleId}
        {...({
          closedby: recordingAction !== null ? "none" : "any",
        } as Record<string, string>)}
      >
        <div className="settings-dialog-panel">
          <div className="settings-dialog-header">
            <h2 id={settingsTitleId} className="settings-dialog-title">
              <Settings size={14} aria-hidden="true" />
              <span>アプリ設定</span>
            </h2>
            <button
              type="button"
              className="icon-btn"
              onClick={() => void closeSettingsDialog()}
              aria-label="アプリ設定を閉じる"
            >
              <X size={14} aria-hidden="true" />
            </button>
          </div>

          <div className="settings-dialog-body">
            <div className="settings-group">
              <span className="settings-group-label">起動設定</span>
              <div className="settings-toggle-row">
                <div className="settings-toggle-copy">
                  <label
                    htmlFor={autostartSwitchId}
                    className="settings-toggle-title"
                  >
                    OS 起動時にバックグラウンドで自動起動
                  </label>
                  <span id={autostartHintId} className="field-hint">
                    ウィンドウを出さず{isMac ? "メニューバー" : "通知領域"}
                    で静かに待機します
                  </span>
                </div>
                <input
                  id={autostartSwitchId}
                  name="autostart_enabled"
                  type="checkbox"
                  autoFocus
                  role={supportsNativeSwitch ? undefined : "switch"}
                  className="nc-toggle compact"
                  checked={autostartEnabled}
                  disabled={autostartBusy}
                  aria-describedby={autostartHintId}
                  onChange={(e) =>
                    void handleToggleAutostart(e.currentTarget.checked)
                  }
                  {...({ switch: "" } as Record<string, string>)}
                />
              </div>
              {autostartError && (
                <div className="error-banner" role="alert">
                  {autostartError}
                </div>
              )}
            </div>

            <div className="settings-group">
              <span className="settings-group-label">
                グローバルショートカット・操作音
              </span>

              <div className="shortcut-row">
                <div className="shortcut-copy">
                  <span className="shortcut-title">ノイズ除去 ON / OFF</span>
                  <kbd
                    className={`shortcut-kbd ${
                      config?.shortcut_toggle_nc ? "" : "is-empty"
                    }`}
                  >
                    {formatShortcutLabel(config?.shortcut_toggle_nc, isMac)}
                  </kbd>
                </div>
                <div className="shortcut-actions">
                  <button
                    type="button"
                    className={`shortcut-record-btn ${
                      recordingAction === "toggle_nc" ? "is-recording" : ""
                    }`}
                    aria-pressed={recordingAction === "toggle_nc"}
                    onClick={() => void startShortcutRecording("toggle_nc")}
                  >
                    <Keyboard size={12} aria-hidden="true" />
                    <span>
                      {recordingAction === "toggle_nc"
                        ? "キーを入力 (Esc取消)"
                        : "設定"}
                    </span>
                    <span className="visually-hidden">
                      （ノイズ除去 ON/OFF）
                    </span>
                  </button>
                  {config?.shortcut_toggle_nc && (
                    <button
                      type="button"
                      className="icon-btn shortcut-clear-btn"
                      onClick={() => void handleClearShortcut("toggle_nc")}
                      aria-label="ノイズ除去 ON/OFF のショートカットを解除"
                    >
                      <X size={12} aria-hidden="true" />
                    </button>
                  )}
                </div>
              </div>

              <div className="shortcut-row">
                <div className="shortcut-copy">
                  <span className="shortcut-title">マイクミュート</span>
                  <kbd
                    className={`shortcut-kbd ${
                      config?.shortcut_toggle_mute ? "" : "is-empty"
                    }`}
                  >
                    {formatShortcutLabel(config?.shortcut_toggle_mute, isMac)}
                  </kbd>
                </div>
                <div className="shortcut-actions">
                  <button
                    type="button"
                    className={`shortcut-record-btn ${
                      recordingAction === "toggle_mute" ? "is-recording" : ""
                    }`}
                    aria-pressed={recordingAction === "toggle_mute"}
                    onClick={() => void startShortcutRecording("toggle_mute")}
                  >
                    <Keyboard size={12} aria-hidden="true" />
                    <span>
                      {recordingAction === "toggle_mute"
                        ? "キーを入力 (Esc取消)"
                        : "設定"}
                    </span>
                    <span className="visually-hidden">（マイクミュート）</span>
                  </button>
                  {config?.shortcut_toggle_mute && (
                    <button
                      type="button"
                      className="icon-btn shortcut-clear-btn"
                      onClick={() => void handleClearShortcut("toggle_mute")}
                      aria-label="マイクミュートのショートカットを解除"
                    >
                      <X size={12} aria-hidden="true" />
                    </button>
                  )}
                </div>
              </div>

              <div className="settings-toggle-row">
                <div className="settings-toggle-copy">
                  <label
                    htmlFor={cueSoundSwitchId}
                    className="settings-toggle-title"
                  >
                    ショートカット操作音
                  </label>
                  <span id={cueSoundHintId} className="field-hint">
                    ホットキーやトレイ切替時に控えめな確認音を鳴らします
                  </span>
                </div>
                <input
                  id={cueSoundSwitchId}
                  name="shortcut_sound_enabled"
                  type="checkbox"
                  role={supportsNativeSwitch ? undefined : "switch"}
                  className="nc-toggle compact"
                  checked={shortcutSoundEnabled}
                  aria-describedby={cueSoundHintId}
                  onChange={(e) =>
                    void handleToggleShortcutSound(e.currentTarget.checked)
                  }
                  {...({ switch: "" } as Record<string, string>)}
                />
              </div>

              {shortcutError && (
                <div className="error-banner" role="alert">
                  {shortcutError}
                </div>
              )}
            </div>

            <div className="settings-group">
              <span className="settings-group-label">自動復旧・診断情報</span>
              <div className="diag-grid">
                <div className="diag-item">
                  <span className="diag-key">パイプライン状態</span>
                  <span className="diag-val">{connStateLabel}</span>
                </div>
                <div className="diag-item">
                  <span className="diag-key">クロック同期補正</span>
                  <span className="diag-val">
                    {(telemetry?.drift_ppm ?? 0) >= 0 ? "+" : ""}
                    {(telemetry?.drift_ppm ?? 0).toFixed(0)} ppm
                  </span>
                </div>
                <div className="diag-item">
                  <span className="diag-key">自動復旧回数</span>
                  <span className="diag-val">
                    {telemetry?.status.reconnect_count ?? 0} 回
                  </span>
                </div>
                <div className="diag-item">
                  <span className="diag-key">サンプルレート</span>
                  <span className="diag-val">
                    {telemetry?.status.input_sample_rate
                      ? `${Math.round(
                          telemetry.status.input_sample_rate / 1000
                        )} kHz`
                      : "—"}
                  </span>
                </div>
                <div className="diag-item">
                  <span className="diag-key">入力ドロップ</span>
                  <span className="diag-val">
                    {telemetry?.status.input_drop_count ?? 0}
                  </span>
                </div>
                <div className="diag-item">
                  <span className="diag-key">出力アンダーラン</span>
                  <span className="diag-val">
                    {telemetry?.status.output_underrun_count ?? 0}
                  </span>
                </div>
              </div>
              <button
                type="button"
                className="btn"
                onClick={() => void handleCopyDiagnostics()}
              >
                {diagCopied ? (
                  <Check size={14} aria-hidden="true" />
                ) : (
                  <ClipboardCopy size={14} aria-hidden="true" />
                )}
                <span>
                  {diagCopied ? "診断情報をコピーしました" : "診断情報をコピー"}
                </span>
              </button>
            </div>
          </div>

          <div className="settings-dialog-footer">
            <span>Shizuka v0.3.0</span>
            <span>ESC または外側クリックで閉じる</span>
          </div>
        </div>
      </dialog>
    </div>
  );
}
