import {
  type CSSProperties,
  useCallback,
  useEffect,
  useId,
  useRef,
  useState,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  AudioLines,
  Check,
  CheckCircle2,
  ClipboardCopy,
  Download,
  ExternalLink,
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
import {
  AgcLiveBadge,
  HeroLiveMeters,
  MicTestLiveMeter,
  SettingsDiagGrid,
  TopBarStatPills,
  VadLiveBadge,
} from "./components/LiveTelemetryWidgets";
import {
  refreshTelemetryNow,
  seedTelemetry,
  setNativeWindowVisible,
  useDiscreteTelemetry,
} from "./telemetryStore";
import type {
  AppConfig,
  AudioTelemetry,
  DeviceListResponse,
  InstallDriverResult,
  ModelVariant,
  ShortcutAction,
  UpdateCheckResult,
  UpdateProgressSnapshot,
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

function applyColorSchemeMeta(
  content: "dark" | "light" | "light dark"
): Promise<void> {
  const meta = document.querySelector<HTMLMetaElement>(
    'meta[name="color-scheme"]'
  );
  if (meta) {
    meta.content = content;
  }
  const nativeTheme =
    content === "dark" || content === "light" ? content : null;
  return invoke("set_window_theme", { theme: nativeTheme })
    .then(() => undefined)
    .catch(() => undefined);
}

function ShizukaMark({
  active,
  muted,
}: {
  active: boolean;
  muted: boolean;
}) {
  const clipId = useId();
  return (
    <svg
      className={`brand-mark ${muted ? "muted" : active ? "active" : ""}`}
      viewBox="0 0 100 100"
      width="26"
      height="26"
      shapeRendering="geometricPrecision"
      aria-hidden="true"
    >
      <defs>
        <clipPath id={clipId}>
          <rect x="4" y="4" width="92" height="92" rx="24" />
        </clipPath>
      </defs>
      <rect className="mark-tile" x="4" y="4" width="92" height="92" rx="24" />
      <g clipPath={`url(#${clipId})`}>
        <path
          className="mark-wave"
          d="M 1 50 L 9 50 C 13.5 50 15.5 30 20 30 C 24.5 30 28.5 70 33 70 C 37.5 70 40.5 40 45 40 C 49 40 51 56 54.5 56 C 58 56 59.5 50 63 50 L 66 50"
          fill="none"
          strokeWidth="8.6"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
        <line
          className="mark-still"
          x1="66"
          y1="50"
          x2="84.5"
          y2="50"
          strokeWidth="8.6"
          strokeLinecap="round"
        />
      </g>
    </svg>
  );
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

function formatIpcError(err: unknown): string {
  return typeof err === "string"
    ? err
    : err instanceof Error
    ? err.message
    : String(err);
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
  const cueSoundSwitchId = `${uid}-cue-sound-switch`;
  const cueSoundHintId = `${uid}-cue-sound-hint`;
  const updateProgressId = `${uid}-update-progress`;

  const settingsDialogRef = useRef<HTMLDialogElement | null>(null);
  const autostartInputRef = useRef<HTMLInputElement | null>(null);
  const lastReconnectCountRef = useRef<number | null>(null);
  const lastConfigRevisionRef = useRef<number | null>(null);
  const recordingActionRef = useRef<ShortcutAction | null>(null);
  const shortcutSubmittingRef = useRef<boolean>(false);
  const ncTogglePendingRef = useRef<boolean>(false);
  const deviceSeqRef = useRef<number>(0);
  const paramSeqRef = useRef<number>(0);
  const configRef = useRef<AppConfig | null>(null);
  const diagCopiedTimerRef = useRef<number | null>(null);
  const updateInstallPollRef = useRef<number | null>(null);

  const telemetry = useDiscreteTelemetry();

  const [appVersion, setAppVersion] = useState<string>("");
  const [devices, setDevices] = useState<DeviceListResponse | null>(null);
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [rescanning, setRescanning] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [monitorClean, setMonitorClean] = useState(true);
  const [autostartEnabled, setAutostartEnabled] = useState(false);
  const [autostartBusy, setAutostartBusy] = useState(false);
  const [autostartError, setAutostartError] = useState<string | null>(null);
  const [recordingAction, setRecordingAction] =
    useState<ShortcutAction | null>(null);
  const [shortcutError, setShortcutError] = useState<string | null>(null);
  const [diagCopied, setDiagCopied] = useState(false);
  const [diagError, setDiagError] = useState<string | null>(null);
  const [updateChecking, setUpdateChecking] = useState(false);
  const [updateInstalling, setUpdateInstalling] = useState(false);
  const [updateInfo, setUpdateInfo] = useState<UpdateCheckResult | null>(null);
  const [updateProgress, setUpdateProgress] = useState<UpdateProgressSnapshot>({
    phase: "idle",
    downloaded_bytes: 0,
    total_bytes: null,
    error: null,
  });
  const [updateError, setUpdateError] = useState<string | null>(null);
  const [installingDriver, setInstallingDriver] = useState(false);
  const [installMessage, setInstallMessage] = useState<{
    ok: boolean;
    text: string;
  } | null>(null);

  recordingActionRef.current = recordingAction;
  configRef.current = config;

  const refreshDevicesAndConfig = useCallback(async (userInitiated = false) => {
    if (userInitiated) {
      setRescanning(true);
      setActionError(null);
    }
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
      setActionError(
        `オーディオデバイスの読み込みに失敗しました: ${formatIpcError(err)}`
      );
    } finally {
      if (userInitiated) {
        setRescanning(false);
      }
    }
  }, []);

  const stopShortcutRecording = useCallback(async () => {
    if (recordingActionRef.current === null) return;
    recordingActionRef.current = null;
    shortcutSubmittingRef.current = false;
    setRecordingAction(null);
    await invoke("set_shortcuts_paused", { paused: false }).catch(() => {});
  }, []);

  const startShortcutRecording = useCallback(
    async (action: ShortcutAction) => {
      setShortcutError(null);
      if (recordingActionRef.current === action) {
        await stopShortcutRecording();
        return;
      }
      try {
        await invoke("set_shortcuts_paused", { paused: true });
        recordingActionRef.current = action;
        shortcutSubmittingRef.current = false;
        setRecordingAction(action);
      } catch (err) {
        setShortcutError(formatIpcError(err));
      }
    },
    [stopShortcutRecording]
  );

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
    setDiagError(null);
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
    // React 18 consumes `autoFocus` on initial mount while `<dialog>` is closed, so move
    // initial focus explicitly onto the primary control inside the modal after showModal().
    autostartInputRef.current?.focus({ preventScroll: true });
  }, []);

  // Capture keyboard shortcut while recording inside Settings dialog
  useEffect(() => {
    if (recordingAction === null) return;

    const onKeyDown = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();

      if (shortcutSubmittingRef.current) {
        return;
      }

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
      shortcutSubmittingRef.current = true;
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
          setShortcutError(formatIpcError(err));
          recordingActionRef.current = null;
          setRecordingAction(null);
          await invoke("set_shortcuts_paused", { paused: false }).catch(
            () => {}
          );
        } finally {
          shortcutSubmittingRef.current = false;
        }
      })();
    };

    window.addEventListener("keydown", onKeyDown, true);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
    };
  }, [recordingAction, stopShortcutRecording]);

  // Cancel shortcut recording if the window is hidden while recording
  useEffect(() => {
    const onVisChange = () => {
      if (document.hidden && recordingActionRef.current !== null) {
        void stopShortcutRecording();
      }
    };
    document.addEventListener("visibilitychange", onVisChange);
    return () => document.removeEventListener("visibilitychange", onVisChange);
  }, [stopShortcutRecording]);

  useEffect(() => {
    const dialog = settingsDialogRef.current;
    if (!dialog) return;

    const onClose = () => {
      dialog.removeAttribute("data-closing");
      dialog.returnValue = "";
      if (recordingActionRef.current !== null) {
        recordingActionRef.current = null;
        shortcutSubmittingRef.current = false;
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

  // Initial bootstrap: load theme, version, devices, config, and first telemetry BEFORE showing
  // the main window so startup never flashes white or placeholder defaults.
  useEffect(() => {
    let active = true;
    const unlisteners: Array<() => void> = [];

    const onStorage = (e: StorageEvent) => {
      if (e.key === COLOR_SCHEME_STORAGE_KEY || e.key === null) {
        const next = readStoredColorScheme();
        void applyColorSchemeMeta(next ?? "light dark");
      }
    };
    window.addEventListener("storage", onStorage);

    void (async () => {
      const initialStored = readStoredColorScheme();
      const [, ver, devs, cfg, autostart, initTelem] = await Promise.all([
        applyColorSchemeMeta(initialStored ?? "light dark"),
        invoke<string>("get_app_version").catch(() => ""),
        invoke<DeviceListResponse>("get_devices").catch(() => null),
        invoke<AppConfig>("get_config").catch(() => null),
        invoke<boolean>("get_autostart_enabled").catch(() => false),
        invoke<AudioTelemetry>("get_telemetry").catch(() => null),
      ]);

      if (!active) return;
      if (ver) setAppVersion(ver);
      if (devs) setDevices(devs);
      if (cfg) setConfig(cfg);
      setAutostartEnabled(autostart);
      if (initTelem) {
        lastReconnectCountRef.current = initTelem.status.reconnect_count;
        lastConfigRevisionRef.current = initTelem.config_revision;
        seedTelemetry(initTelem);
      }

      const isVisible = await invoke<boolean>("frontend_ready").catch(
        () => true
      );
      if (!active) return;
      setNativeWindowVisible(isVisible);

      try {
        const u1 = await listen<boolean>("shizuka-window-visibility", (ev) => {
          const vis = Boolean(ev.payload);
          setNativeWindowVisible(vis);
          if (!vis && recordingActionRef.current !== null) {
            void stopShortcutRecording();
          }
        });
        const u2 = await listen("shizuka-state-changed", () => {
          refreshTelemetryNow();
        });
        if (!active) {
          u1();
          u2();
        } else {
          unlisteners.push(u1, u2);
        }
      } catch {
        // Fallback: keep polling tied to document visibility if event listener is unavailable
      }
    })();

    return () => {
      active = false;
      window.removeEventListener("storage", onStorage);
      setNativeWindowVisible(false);
      for (const u of unlisteners) {
        u();
      }
      if (diagCopiedTimerRef.current !== null) {
        window.clearTimeout(diagCopiedTimerRef.current);
      }
      if (updateInstallPollRef.current !== null) {
        window.clearInterval(updateInstallPollRef.current);
      }
    };
  }, [stopShortcutRecording]);

  // React to discrete backend state transitions (tray/hotkey toggles, auto-reconnects, mic profile restore)
  useEffect(() => {
    if (!telemetry) return;

    if (!ncTogglePendingRef.current) {
      setConfig((prev) =>
        prev && prev.nc_enabled !== telemetry.nc_enabled
          ? { ...prev, nc_enabled: telemetry.nc_enabled }
          : prev
      );
    }

    if (
      lastReconnectCountRef.current !== null &&
      telemetry.reconnect_count !== lastReconnectCountRef.current
    ) {
      lastReconnectCountRef.current = telemetry.reconnect_count;
      void refreshDevicesAndConfig(false);
    } else if (
      lastConfigRevisionRef.current !== null &&
      telemetry.config_revision !== lastConfigRevisionRef.current
    ) {
      lastConfigRevisionRef.current = telemetry.config_revision;
      const reqSeq = ++paramSeqRef.current;
      void invoke<AppConfig>("get_config")
        .then((fresh) => {
          if (reqSeq === paramSeqRef.current) {
            setConfig(fresh);
          }
        })
        .catch(() => {});
    }
  }, [telemetry, refreshDevicesAndConfig]);

  const ncEnabled = ncTogglePendingRef.current
    ? config?.nc_enabled ?? true
    : telemetry?.nc_enabled ?? config?.nc_enabled ?? true;
  const micMuted = telemetry?.mic_muted ?? false;

  const handleToggleNc = async (next: boolean) => {
    setActionError(null);
    ncTogglePendingRef.current = true;
    setConfig((prev) => (prev ? { ...prev, nc_enabled: next } : prev));
    try {
      await invoke("set_nc_enabled", { enabled: next });
      refreshTelemetryNow();
    } catch (err) {
      setConfig((prev) => (prev ? { ...prev, nc_enabled: !next } : prev));
      setActionError(`ノイズ除去の切替に失敗しました: ${formatIpcError(err)}`);
    } finally {
      ncTogglePendingRef.current = false;
    }
  };

  const handleToggleMute = async () => {
    setActionError(null);
    const next = !micMuted;
    try {
      await invoke("set_mic_muted", { muted: next });
      refreshTelemetryNow();
    } catch (err) {
      setActionError(`ミュート切替に失敗しました: ${formatIpcError(err)}`);
    }
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
      void applyColorSchemeMeta(target);
    } else {
      try {
        localStorage.removeItem(COLOR_SCHEME_STORAGE_KEY);
      } catch {
        // Ignore storage error
      }
      void applyColorSchemeMeta("light dark");
    }
  };

  const handleDeviceChange = async (
    nextIn: string | null,
    nextOut: string | null,
    nextMon: string | null
  ) => {
    setActionError(null);
    const seq = ++deviceSeqRef.current;
    const prevSnapshot = configRef.current;
    setConfig((prev) =>
      prev
        ? {
            ...prev,
            input_device: nextIn,
            output_device: nextOut,
            monitor_device: nextMon,
          }
        : prev
    );
    try {
      const synced = await invoke<AppConfig>("select_devices", {
        inputDevice: nextIn,
        outputDevice: nextOut,
        monitorDevice: nextMon,
      });
      if (seq === deviceSeqRef.current) {
        setConfig(synced);
      }
    } catch (err) {
      if (seq === deviceSeqRef.current && prevSnapshot) {
        setConfig(prevSnapshot);
      }
      setActionError(
        `デバイスの切替に失敗しました: ${formatIpcError(err)}`
      );
    }
  };

  const handleModelVariantChange = async (nextVariant: ModelVariant) => {
    setActionError(null);
    const prevVariant = configRef.current?.model_variant;
    setConfig((prev) =>
      prev ? { ...prev, model_variant: nextVariant } : prev
    );
    try {
      await invoke("set_model_variant", { modelVariant: nextVariant });
    } catch (err) {
      if (prevVariant) {
        setConfig((prev) =>
          prev ? { ...prev, model_variant: prevVariant } : prev
        );
      }
      setActionError(
        `AI モデルの切替に失敗しました: ${formatIpcError(err)}`
      );
    }
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
    const cur = configRef.current;
    if (!cur) return;
    setActionError(null);
    const seq = ++paramSeqRef.current;
    const minThresh = cur.min_db_thresh;
    setConfig((prev) =>
      prev
        ? {
            ...prev,
            atten_lim_db: attenLimDb,
            post_filter_beta: postFilterBeta,
            input_gain_db: inputGainDb,
            vad_gate_enabled: vadGateEnabled,
            vad_threshold_db: vadThresholdDb,
            hpf_enabled: hpfEnabled,
            agc_enabled: agcEnabled,
          }
        : prev
    );
    try {
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
    } catch (err) {
      if (seq === paramSeqRef.current) {
        setActionError(`音質パラメータの反映に失敗しました: ${formatIpcError(err)}`);
        void invoke<AppConfig>("get_config")
          .then(setConfig)
          .catch(() => {});
      }
    }
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
      setShortcutError(formatIpcError(err));
    }
  };

  const handleToggleShortcutSound = async (enabled: boolean) => {
    setShortcutError(null);
    setConfig((prev) =>
      prev ? { ...prev, shortcut_sound_enabled: enabled } : prev
    );
    try {
      await invoke("set_shortcut_sound_enabled", { enabled });
    } catch (err) {
      setConfig((prev) =>
        prev ? { ...prev, shortcut_sound_enabled: !enabled } : prev
      );
      setShortcutError(formatIpcError(err));
    }
  };

  const handleToggleLiveMonitor = async () => {
    setActionError(null);
    const cur = telemetry?.live_monitor_enabled ?? false;
    try {
      await invoke("set_live_monitor", { enabled: !cur });
      refreshTelemetryNow();
    } catch (err) {
      setActionError(
        `マイクテストの切替に失敗しました: ${formatIpcError(err)}`
      );
    }
  };

  const handleMonitorModeChange = async (clean: boolean) => {
    setActionError(null);
    setMonitorClean(clean);
    try {
      await invoke("set_monitor_mode", { clean });
    } catch (err) {
      setMonitorClean(!clean);
      setActionError(
        `モニターモードの切替に失敗しました: ${formatIpcError(err)}`
      );
    }
  };

  const handleMonitorVolumeChange = async (pct: number) => {
    const clampedPct = Math.max(0, Math.min(100, pct));
    const vol = clampedPct / 100;
    setConfig((prev) => (prev ? { ...prev, monitor_volume: vol } : prev));
    try {
      await invoke("set_monitor_volume", { volume: vol });
    } catch (err) {
      setActionError(`モニター音量の変更に失敗しました: ${formatIpcError(err)}`);
    }
  };

  const handleQuit = async () => {
    try {
      await invoke("quit_app");
    } catch (err) {
      setActionError(`終了処理に失敗しました: ${formatIpcError(err)}`);
    }
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
      setAutostartError(formatIpcError(err));
    } finally {
      setAutostartBusy(false);
    }
  };

  const handleCopyDiagnostics = async () => {
    setDiagError(null);
    try {
      const text = await invoke<string>("get_diagnostics_text");
      await navigator.clipboard.writeText(text);
      setDiagCopied(true);
      if (diagCopiedTimerRef.current !== null) {
        window.clearTimeout(diagCopiedTimerRef.current);
      }
      diagCopiedTimerRef.current = window.setTimeout(() => {
        diagCopiedTimerRef.current = null;
        setDiagCopied(false);
      }, 2200);
    } catch (err) {
      setDiagError(
        `クリップボードへのコピーに失敗しました: ${formatIpcError(err)}`
      );
    }
  };

  const handleCheckForUpdate = useCallback(async (silent = false) => {
    setUpdateChecking(true);
    if (!silent) {
      setUpdateError(null);
    }
    try {
      const res = await invoke<UpdateCheckResult>("check_for_update");
      setUpdateInfo(res);
    } catch (err) {
      if (!silent) {
        setUpdateError(formatIpcError(err));
      }
    } finally {
      setUpdateChecking(false);
    }
  }, []);

  const handleInstallUpdate = async () => {
    setUpdateInstalling(true);
    setUpdateError(null);
    setUpdateProgress({
      phase: "downloading",
      downloaded_bytes: 0,
      total_bytes: null,
      error: null,
    });
    if (updateInstallPollRef.current !== null) {
      window.clearInterval(updateInstallPollRef.current);
    }
    updateInstallPollRef.current = window.setInterval(async () => {
      try {
        const snap = await invoke<UpdateProgressSnapshot>(
          "get_update_progress"
        );
        setUpdateProgress(snap);
      } catch {
        // Ignore transient IPC error while restarting
      }
    }, 200);
    try {
      await invoke("install_update");
    } catch (err) {
      setUpdateError(formatIpcError(err));
      setUpdateProgress((prev) => ({ ...prev, phase: "error" }));
      setUpdateInstalling(false);
    } finally {
      if (updateInstallPollRef.current !== null) {
        window.clearInterval(updateInstallPollRef.current);
        updateInstallPollRef.current = null;
      }
    }
  };

  const handleOpenReleasesPage = async () => {
    setUpdateError(null);
    try {
      await invoke("open_releases_page");
    } catch (err) {
      setUpdateError(formatIpcError(err));
    }
  };

  // Quiet one-shot startup update check (5s after mount so audio startup is unaffected)
  useEffect(() => {
    const timer = window.setTimeout(() => {
      void handleCheckForUpdate(true);
    }, 5000);
    return () => window.clearTimeout(timer);
  }, [handleCheckForUpdate]);

  const handleInstallVirtualDriver = async () => {
    setInstallingDriver(true);
    setInstallMessage(null);
    try {
      const res = await invoke<InstallDriverResult>("install_virtual_driver");
      setInstallMessage({ ok: res.success, text: res.message });
      await refreshDevicesAndConfig(false);
    } catch (err) {
      setInstallMessage({
        ok: false,
        text: formatIpcError(err),
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
  const liveMonitorEnabled = telemetry?.live_monitor_enabled ?? false;
  const monVolPct = Math.round(
    (config?.monitor_volume ?? telemetry?.monitor_volume ?? 0.8) * 100
  );
  const isMac = devices?.platform === "macos";
  const shortcutSoundEnabled = config?.shortcut_sound_enabled ?? true;
  const connState = telemetry?.connection_state ?? "running";
  const displayVersion = updateInfo?.current_version ?? appVersion;

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

  // Detect configured devices that are temporarily unplugged so <select> never falsely displays
  // "システム既定" while the backend is waiting to auto-reconnect the configured hardware.
  const configuredInputMissing = Boolean(
    config?.input_device &&
      devices &&
      !devices.inputs.some((d) => d.name === config.input_device)
  );
  const configuredOutputMissing = Boolean(
    config?.output_device &&
      devices &&
      !devices.outputs.some((d) => d.name === config.output_device)
  );
  const physicalOutputs =
    devices?.outputs.filter((d) => !d.is_virtual_cable) ?? [];
  const configuredMonitorMissing = Boolean(
    config?.monitor_device &&
      devices &&
      !physicalOutputs.some((d) => d.name === config.monitor_device)
  );

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
          <TopBarStatPills />
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
            className={`icon-btn ${rescanning ? "is-busy" : ""}`}
            disabled={rescanning}
            onClick={() => void refreshDevicesAndConfig(true)}
            aria-label="オーディオデバイスを再スキャン"
          >
            <RefreshCw size={14} aria-hidden="true" />
          </button>
          <button
            type="button"
            className="icon-btn settings-icon-btn"
            onClick={openSettingsDialog}
            aria-label={
              updateInfo?.available
                ? "アプリ設定を開く（新しいアップデートがあります）"
                : "アプリ設定を開く"
            }
          >
            <Settings size={14} aria-hidden="true" />
            {updateInfo?.available && (
              <span className="update-badge-dot" aria-hidden="true" />
            )}
          </button>
        </div>
      </header>

      <main className="main-stack">
        {connState === "waiting_device" && (
          <div className="status-banner" role="status">
            <span className="status-banner-dot" aria-hidden="true" />
            <span>
              <strong>マイク再接続待ち：</strong>
              {telemetry?.waiting_device_name
                ? `「${telemetry.waiting_device_name}」が接続されると自動で再開します。`
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

        {(actionError || telemetry?.last_error) && (
          <div className="error-banner" role="alert">
            <TriangleAlert
              size={13}
              aria-hidden="true"
              style={{
                verticalAlign: "text-bottom",
                marginInlineEnd: "0.3rem",
              }}
            />
            {actionError ?? telemetry?.last_error}
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

          <HeroLiveMeters
            inputMeterId={inputMeterId}
            outputMeterId={outputMeterId}
            ncEnabled={ncEnabled}
            micMuted={micMuted}
          />
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
              {configuredInputMissing && config?.input_device && (
                <option value={config.input_device}>
                  {config.input_device} (未接続)
                </option>
              )}
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
              <option value="">出力しない (マイクテストのみ)</option>
              {configuredOutputMissing && config?.output_device && (
                <option value={config.output_device}>
                  {config.output_device} (未接続)
                </option>
              )}
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

            {/* Voice Conditioning Toggles: HPF 65Hz & VAD-Gated AGC */}
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
                    {agcEnabled && <AgcLiveBadge />}
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
                  <VadLiveBadge
                    vadGateEnabled={vadGateEnabled}
                    ncEnabled={ncEnabled}
                  />
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
              <MicTestLiveMeter
                monMeterId={monMeterId}
                liveMonitorEnabled={liveMonitorEnabled}
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
                {configuredMonitorMissing && config?.monitor_device && (
                  <option value={config.monitor_device}>
                    {config.monitor_device} (未接続)
                  </option>
                )}
                {physicalOutputs.map((d) => (
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
                </div>
                <input
                  ref={autostartInputRef}
                  id={autostartSwitchId}
                  name="autostart_enabled"
                  type="checkbox"
                  role={supportsNativeSwitch ? undefined : "switch"}
                  className="nc-toggle compact"
                  checked={autostartEnabled}
                  disabled={autostartBusy}
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
                    ホットキーや{isMac ? "メニューバー" : "通知領域"}
                    からの切替時に控えめな確認音を鳴らします
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
              <SettingsDiagGrid connState={connState} />
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
              {diagError && (
                <div className="error-banner" role="alert">
                  {diagError}
                </div>
              )}
            </div>

            <div className="settings-group">
              <span className="settings-group-label">アップデート</span>
              <div className="update-card">
                <div className="update-card-top">
                  <div className="update-copy">
                    <span className="update-title">
                      {displayVersion ? `Shizuka v${displayVersion}` : "Shizuka"}
                    </span>
                    <span className="field-hint" role="status">
                      {updateChecking
                        ? "最新バージョンを確認しています..."
                        : updateInfo?.available
                        ? `v${updateInfo.latest_version} が利用可能です${
                            updateInfo.is_portable ? "（ポータブル版）" : ""
                          }`
                        : updateInfo
                        ? "最新バージョンです"
                        : "最新版をオンラインで確認できます"}
                    </span>
                  </div>
                  <button
                    type="button"
                    className="shortcut-record-btn"
                    disabled={updateChecking || updateInstalling}
                    onClick={() => void handleCheckForUpdate(false)}
                  >
                    {updateChecking ? (
                      <progress
                        className="loading-spinner"
                        aria-label="アップデートを確認中"
                      />
                    ) : (
                      <RefreshCw size={12} aria-hidden="true" />
                    )}
                    <span>更新を確認</span>
                  </button>
                </div>

                {updateInfo?.available && (
                  <div className="update-action-box">
                    {updateInstalling && (
                      <div className="update-progress-wrap">
                        <div className="slider-top">
                          <label htmlFor={updateProgressId}>
                            {updateProgress.phase === "installing"
                              ? "インストールを適用しています..."
                              : "アップデートをダウンロード中..."}
                          </label>
                          {updateProgress.total_bytes &&
                            updateProgress.total_bytes > 0 && (
                              <span className="slider-val">
                                {Math.min(
                                  100,
                                  Math.round(
                                    (updateProgress.downloaded_bytes /
                                      updateProgress.total_bytes) *
                                      100
                                  )
                                )}
                                %
                              </span>
                            )}
                        </div>
                        <progress
                          id={updateProgressId}
                          className="update-progress"
                          max={
                            updateProgress.phase === "downloading" &&
                            updateProgress.total_bytes &&
                            updateProgress.total_bytes > 0
                              ? updateProgress.total_bytes
                              : undefined
                          }
                          value={
                            updateProgress.phase === "downloading" &&
                            updateProgress.total_bytes &&
                            updateProgress.total_bytes > 0
                              ? updateProgress.downloaded_bytes
                              : undefined
                          }
                        />
                      </div>
                    )}

                    {updateInfo.is_portable ? (
                      <>
                        <span className="field-hint">
                          ポータブル版は自動上書きに対応していないため、GitHub
                          Releases から最新版を取得してください。
                        </span>
                        <button
                          type="button"
                          className="btn primary"
                          onClick={() => void handleOpenReleasesPage()}
                        >
                          <ExternalLink size={14} aria-hidden="true" />
                          <span>Releases ページを開く</span>
                        </button>
                      </>
                    ) : (
                      <button
                        type="button"
                        className="btn primary"
                        disabled={updateInstalling}
                        onClick={() => void handleInstallUpdate()}
                      >
                        <Download size={14} aria-hidden="true" />
                        <span>
                          {updateInstalling
                            ? "アップデートを適用中..."
                            : `v${updateInfo.latest_version} へ今すぐアップデート`}
                        </span>
                      </button>
                    )}
                  </div>
                )}

                {updateError && (
                  <div className="error-banner" role="alert">
                    {updateError}
                  </div>
                )}
              </div>
            </div>
          </div>

          <div className="settings-dialog-footer">
            <span>
              {displayVersion ? `Shizuka v${displayVersion}` : "Shizuka"}
            </span>
          </div>
        </div>
      </dialog>
    </div>
  );
}
