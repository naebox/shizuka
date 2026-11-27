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
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  ArrowLeft,
  AudioLines,
  Check,
  CheckCircle2,
  ClipboardCopy,
  Download,
  ExternalLink,
  FileText,
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
  AecLiveBadge,
  AgcLiveBadge,
  HeroInlineStats,
  HeroLiveMeters,
  MicTestLiveMeter,
  SettingsDiagGrid,
  VadLiveBadge,
  VoiceAutoLearnProgress,
  VoiceGateLiveBadge,
} from "./components/LiveTelemetryWidgets";
import { detectBrowserLocale, getMessages, type UiMessages } from "./i18n";
import {
  getBundledChangelog,
  getReleaseItems,
  getReleaseTitle,
  parseSingleUpdateNotes,
  type ReleaseNoteBullet,
  tokenizeInlineMarkdown,
} from "./releaseNotes";
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
  LanguagePreference,
  Locale,
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
  e: KeyboardEvent,
  t: UiMessages
): { ok: true; value: string } | { ok: false; error: string } | null {
  if (MODIFIER_CODES.has(e.code)) {
    return null;
  }
  if (!isSupportedPrimaryCode(e.code)) {
    return {
      ok: false,
      error: t.errors.unsupportedKey,
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
      error: t.errors.unmodifiedKeyNotAllowed,
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
  isMac: boolean,
  unassignedLabel: string
): string {
  if (!accelerator) return unassignedLabel;
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

function applyDocumentLocale(loc: Locale) {
  if (typeof document !== "undefined") {
    document.documentElement.lang = loc;
    document.title = getMessages(loc).docTitle;
  }
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
      width="20"
      height="20"
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

function InlineFormattedText({ text }: { text: string }) {
  const segments = tokenizeInlineMarkdown(text);
  return (
    <>
      {segments.map((seg, i) => {
        if (seg.type === "code") {
          return (
            <code key={i} className="release-inline-code">
              {seg.value}
            </code>
          );
        }
        if (seg.type === "strong") {
          return (
            <strong key={i}>
              {seg.children.map((child, j) =>
                child.type === "code" ? (
                  <code key={j} className="release-inline-code">
                    {child.value}
                  </code>
                ) : (
                  child.value
                )
              )}
            </strong>
          );
        }
        return seg.value;
      })}
    </>
  );
}

function ReleaseBulletList({
  items,
  compact = false,
}: {
  items: ReleaseNoteBullet[];
  compact?: boolean;
}) {
  return (
    <ul className={`release-bullet-list ${compact ? "is-compact" : ""}`}>
      {items.map((item, idx) => (
        <li key={idx} className="release-bullet-item">
          <span className="release-bullet-headline">
            <InlineFormattedText text={item.text} />
          </span>
          {item.subItems.length > 0 && (
            <ul className="release-subbullet-list">
              {item.subItems.map((sub, subIdx) => (
                <li key={subIdx} className="release-subbullet-item">
                  <InlineFormattedText text={sub} />
                </li>
              ))}
            </ul>
          )}
        </li>
      ))}
    </ul>
  );
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
  const aecSwitchId = `${uid}-aec-switch`;
  const aecHintId = `${uid}-aec-hint`;
  const aecRefSelectId = `${uid}-aec-ref-select`;
  const aecRefHintId = `${uid}-aec-ref-hint`;
  const vadSwitchId = `${uid}-vad-switch`;
  const vadSliderId = `${uid}-vad-slider`;
  const vadHintId = `${uid}-vad-hint`;
  const voiceGateSwitchId = `${uid}-vgate-switch`;
  const voiceGateHintId = `${uid}-vgate-hint`;
  const voiceGateSliderId = `${uid}-vgate-slider`;
  const voiceGateSliderHintId = `${uid}-vgate-slider-hint`;
  const voiceEnrollProgressId = `${uid}-venroll-progress`;
  const inputMeterId = `${uid}-in-meter`;
  const outputMeterId = `${uid}-out-meter`;
  const monMeterId = `${uid}-mon-meter`;
  const monCleanId = `${uid}-mon-clean`;
  const monRawId = `${uid}-mon-raw`;
  const monVolSliderId = `${uid}-mon-vol-slider`;
  const settingsTitleId = `${uid}-settings-title`;
  const langAutoId = `${uid}-lang-auto`;
  const langJaId = `${uid}-lang-ja`;
  const langEnId = `${uid}-lang-en`;
  const autostartSwitchId = `${uid}-autostart-switch`;
  const cueSoundSwitchId = `${uid}-cue-sound-switch`;
  const cueSoundHintId = `${uid}-cue-sound-hint`;
  const updateProgressId = `${uid}-update-progress`;

  const settingsDialogRef = useRef<HTMLDialogElement | null>(null);
  const activeLangInputRef = useRef<HTMLInputElement | null>(null);
  const autostartInputRef = useRef<HTMLInputElement | null>(null);
  const releaseNotesTriggerRef = useRef<HTMLButtonElement | null>(null);
  const releaseNotesBackRef = useRef<HTMLButtonElement | null>(null);
  const settingsViewRef = useRef<"main" | "release_notes">("main");
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

  const [locale, setLocale] = useState<Locale>(() => detectBrowserLocale());
  const t = getMessages(locale);
  const tRef = useRef<UiMessages>(t);
  tRef.current = t;

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
  const [settingsView, setSettingsView] = useState<"main" | "release_notes">(
    "main"
  );
  const [installingDriver, setInstallingDriver] = useState(false);
  const [installMessage, setInstallMessage] = useState<{
    ok: boolean;
    text: string;
  } | null>(null);

  recordingActionRef.current = recordingAction;
  settingsViewRef.current = settingsView;
  configRef.current = config;

  useEffect(() => {
    applyDocumentLocale(locale);
  }, [locale]);

  const refreshDevicesAndConfig = useCallback(async (userInitiated = false) => {
    if (userInitiated) {
      setRescanning(true);
      setActionError(null);
    }
    try {
      const [devs, cfg, autostart, resolvedLoc] = await Promise.all([
        invoke<DeviceListResponse>("get_devices"),
        invoke<AppConfig>("get_config"),
        invoke<boolean>("get_autostart_enabled").catch(() => false),
        invoke<Locale>("get_resolved_locale").catch(() => null),
      ]);
      setDevices(devs);
      setConfig(cfg);
      setAutostartEnabled(autostart);
      if (resolvedLoc) {
        setLocale(resolvedLoc);
      }
    } catch (err) {
      setActionError(
        tRef.current.errors.loadDevicesFailed(formatIpcError(err))
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

  const openReleaseNotesView = useCallback(() => {
    settingsViewRef.current = "release_notes";
    setSettingsView("release_notes");
    window.requestAnimationFrame(() => {
      releaseNotesBackRef.current?.focus({ preventScroll: true });
    });
  }, []);

  const returnToSettingsMain = useCallback(() => {
    settingsViewRef.current = "main";
    setSettingsView("main");
    window.requestAnimationFrame(() => {
      releaseNotesTriggerRef.current?.focus({ preventScroll: true });
    });
  }, []);

  const openSettingsDialog = useCallback(() => {
    const dialog = settingsDialogRef.current;
    if (!dialog || dialog.open) return;
    dialog.removeAttribute("data-closing");
    settingsViewRef.current = "main";
    setSettingsView("main");
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
    (activeLangInputRef.current ?? autostartInputRef.current)?.focus({
      preventScroll: true,
    });
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

      const parsed = buildAcceleratorFromKeyEvent(e, tRef.current);
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
      settingsViewRef.current = "main";
      setSettingsView("main");
      if (recordingActionRef.current !== null) {
        recordingActionRef.current = null;
        shortcutSubmittingRef.current = false;
        setRecordingAction(null);
        void invoke("set_shortcuts_paused", { paused: false }).catch(() => {});
      }
    };
    dialog.addEventListener("close", onClose);

    const onDialogKeyDown = (e: KeyboardEvent) => {
      if (
        (e.code === "Escape" || e.key === "Escape") &&
        recordingActionRef.current === null &&
        settingsViewRef.current === "release_notes"
      ) {
        e.preventDefault();
        e.stopPropagation();
        returnToSettingsMain();
      }
    };
    dialog.addEventListener("keydown", onDialogKeyDown);

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
      if (settingsViewRef.current === "release_notes") {
        e.preventDefault();
        returnToSettingsMain();
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

    // WKWebView fallback: Safari ignores `overscroll-behavior: contain` on non-scrollable
    // areas (backdrop/header/footer) and chains new wheel gestures started at scroll boundaries.
    const canScrollY = (el: HTMLElement, dy: number) =>
      (dy < 0 && el.scrollTop > 0) ||
      (dy > 0 && el.scrollTop + el.clientHeight < el.scrollHeight - 1);
    const onWheel = (e: WheelEvent) => {
      if (e.ctrlKey || !dialog.open) return;
      for (
        let el = e.target instanceof HTMLElement ? e.target : null;
        el && el !== dialog;
        el = el.parentElement
      ) {
        if (
          (el.classList.contains("update-notes-preview") ||
            el.classList.contains("settings-dialog-body")) &&
          canScrollY(el, e.deltaY)
        ) {
          return;
        }
      }
      e.preventDefault();
    };
    dialog.addEventListener("wheel", onWheel, { passive: false });

    return () => {
      dialog.removeEventListener("close", onClose);
      dialog.removeEventListener("keydown", onDialogKeyDown);
      dialog.removeEventListener("cancel", onCancel);
      dialog.removeEventListener("wheel", onWheel);
      if (!supportsClosedBy) {
        dialog.removeEventListener("pointerdown", onPointerDown);
        dialog.removeEventListener("pointerup", onPointerUp);
      }
    };
  }, [closeSettingsDialog, returnToSettingsMain, stopShortcutRecording]);

  // Initial bootstrap: load theme, locale, version, devices, config, and first telemetry BEFORE showing
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
      const [, ver, devs, cfg, resolvedLoc, autostart, initTelem] =
        await Promise.all([
          applyColorSchemeMeta(initialStored ?? "light dark"),
          invoke<string>("get_app_version").catch(() => ""),
          invoke<DeviceListResponse>("get_devices").catch(() => null),
          invoke<AppConfig>("get_config").catch(() => null),
          invoke<Locale>("get_resolved_locale").catch(() => null),
          invoke<boolean>("get_autostart_enabled").catch(() => false),
          invoke<AudioTelemetry>("get_telemetry").catch(() => null),
        ]);

      if (!active) return;
      if (resolvedLoc) {
        applyDocumentLocale(resolvedLoc);
        setLocale(resolvedLoc);
      }
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
      setActionError(t.errors.toggleNcFailed(formatIpcError(err)));
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
      setActionError(t.errors.toggleMuteFailed(formatIpcError(err)));
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

  const handleLanguageChange = async (nextPref: LanguagePreference) => {
    setAutostartError(null);
    const prevPref = configRef.current?.language ?? "auto";
    const prevLocale = locale;
    const optimisticLocale: Locale =
      nextPref === "auto" ? detectBrowserLocale() : nextPref;

    setConfig((prev) => (prev ? { ...prev, language: nextPref } : prev));
    setLocale(optimisticLocale);

    try {
      const resolved = await invoke<Locale>("set_language", {
        language: nextPref,
      });
      setLocale(resolved);
      const warn = await invoke<string | null>("get_shortcut_warning").catch(
        () => null
      );
      setShortcutError(warn);
    } catch (err) {
      setConfig((prev) => (prev ? { ...prev, language: prevPref } : prev));
      setLocale(prevLocale);
      setAutostartError(
        tRef.current.errors.switchLanguageFailed(formatIpcError(err))
      );
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
      setActionError(t.errors.switchDeviceFailed(formatIpcError(err)));
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
      setActionError(t.errors.switchModelFailed(formatIpcError(err)));
    }
  };

  const handleParamChange = async (
    attenLimDb: number,
    postFilterBeta: number,
    inputGainDb: number,
    vadGateEnabled: boolean,
    vadThresholdDb: number,
    hpfEnabled: boolean,
    agcEnabled: boolean,
    aecEnabled: boolean
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
            aec_enabled: aecEnabled,
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
        aecEnabled,
      });
    } catch (err) {
      if (seq === paramSeqRef.current) {
        setActionError(t.errors.updateDspFailed(formatIpcError(err)));
        void invoke<AppConfig>("get_config")
          .then(setConfig)
          .catch(() => {});
      }
    }
  };

  const handleAecReferenceChange = async (nextRef: string | null) => {
    setActionError(null);
    const seq = ++deviceSeqRef.current;
    const prevSnapshot = configRef.current;
    setConfig((prev) =>
      prev ? { ...prev, aec_reference_device: nextRef } : prev
    );
    try {
      const synced = await invoke<AppConfig>("set_aec_reference_device", {
        referenceDevice: nextRef,
      });
      if (seq === deviceSeqRef.current) {
        setConfig(synced);
      }
      refreshTelemetryNow();
    } catch (err) {
      if (seq === deviceSeqRef.current && prevSnapshot) {
        setConfig(prevSnapshot);
      }
      setActionError(t.errors.switchSpeakerFailed(formatIpcError(err)));
    }
  };

  const handleVoiceGateChange = async (
    nextEnabled: boolean,
    nextThreshold: number
  ) => {
    setActionError(null);
    const seq = ++paramSeqRef.current;
    const prevSnapshot = configRef.current;
    setConfig((prev) =>
      prev
        ? {
            ...prev,
            voice_gate_enabled: nextEnabled,
            voice_gate_threshold: nextThreshold,
          }
        : prev
    );
    try {
      const synced = await invoke<AppConfig>("set_voice_gate_config", {
        enabled: nextEnabled,
        threshold: nextThreshold,
      });
      if (seq === paramSeqRef.current) {
        setConfig(synced);
      }
      refreshTelemetryNow();
    } catch (err) {
      if (seq === paramSeqRef.current && prevSnapshot) {
        setConfig(prevSnapshot);
      }
      setActionError(t.errors.updateVoiceGateFailed(formatIpcError(err)));
    }
  };

  const handleResetVoiceProfile = async () => {
    setActionError(null);
    try {
      const synced = await invoke<AppConfig>("reset_voice_profile");
      setConfig(synced);
      refreshTelemetryNow();
    } catch (err) {
      setActionError(t.errors.resetVoiceProfileFailed(formatIpcError(err)));
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
      setActionError(t.errors.toggleMicTestFailed(formatIpcError(err)));
    }
  };

  const handleMonitorModeChange = async (clean: boolean) => {
    setActionError(null);
    setMonitorClean(clean);
    try {
      await invoke("set_monitor_mode", { clean });
    } catch (err) {
      setMonitorClean(!clean);
      setActionError(t.errors.switchMonitorModeFailed(formatIpcError(err)));
    }
  };

  const handleMonitorVolumeChange = async (pct: number) => {
    const clampedPct = Math.max(0, Math.min(100, pct));
    const vol = clampedPct / 100;
    setConfig((prev) => (prev ? { ...prev, monitor_volume: vol } : prev));
    try {
      await invoke("set_monitor_volume", { volume: vol });
    } catch (err) {
      setActionError(t.errors.changeMonitorVolFailed(formatIpcError(err)));
    }
  };

  const handleQuit = async () => {
    try {
      await invoke("quit_app");
    } catch (err) {
      setActionError(t.errors.quitFailed(formatIpcError(err)));
    }
  };

  const handleMinimizeWindow = () => {
    void getCurrentWindow()
      .minimize()
      .catch(() => {});
  };

  const handleCloseWindow = () => {
    void getCurrentWindow()
      .close()
      .catch(() => {});
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
      setDiagError(t.errors.copyClipboardFailed(formatIpcError(err)));
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

  const languagePref: LanguagePreference = config?.language ?? "auto";
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
  const aecEnabled = config?.aec_enabled ?? telemetry?.aec_enabled ?? false;
  const hasVoiceProfile =
    telemetry?.has_voice_profile ?? Boolean(config?.voice_profile?.length);
  const voiceGateEnabled =
    config?.voice_gate_enabled ?? telemetry?.voice_gate_enabled ?? false;
  const voiceGateThresh = Math.max(
    0.16,
    Math.min(0.4, config?.voice_gate_threshold ?? 0.28)
  );
  const voiceStrictPct = Math.round(((voiceGateThresh - 0.16) / 0.24) * 100);
  const voiceGateValueText =
    voiceStrictPct < 35
      ? t.tuning.voiceStrictnessRelaxed(voiceStrictPct)
      : voiceStrictPct > 65
      ? t.tuning.voiceStrictnessStrict(voiceStrictPct)
      : t.tuning.voiceStrictnessBalanced(voiceStrictPct);
  const liveMonitorEnabled = telemetry?.live_monitor_enabled ?? false;
  const monVolPct = Math.round(
    (config?.monitor_volume ?? telemetry?.monitor_volume ?? 0.8) * 100
  );
  const isMac = devices
    ? devices.platform === "macos"
    : typeof navigator !== "undefined" &&
      /Mac|iPhone|iPod|iPad/i.test(navigator.userAgent);
  const shortcutSoundEnabled = config?.shortcut_sound_enabled ?? true;
  const connState = telemetry?.connection_state ?? "running";
  const displayVersion = updateInfo?.current_version ?? appVersion;
  const bundledChangelog = getBundledChangelog();
  const pendingUpdateEntry =
    updateInfo?.available && updateInfo.latest_version
      ? parseSingleUpdateNotes(
          updateInfo.latest_version,
          updateInfo.date,
          updateInfo.notes
        ) ??
        bundledChangelog.find(
          (entry) => entry.version === updateInfo.latest_version
        ) ??
        null
      : null;
  const pendingUpdateItems = pendingUpdateEntry
    ? getReleaseItems(pendingUpdateEntry, locale)
    : [];
  const allReleaseEntries =
    pendingUpdateEntry &&
    !bundledChangelog.some(
      (entry) => entry.version === pendingUpdateEntry.version
    )
      ? [pendingUpdateEntry, ...bundledChangelog]
      : bundledChangelog;

  const activePreset =
    attenDb <= 30 ? "natural" : attenDb >= 90 ? "max" : "balanced";
  const attenValueText =
    attenDb >= 100 ? t.tuning.attenUnlimited : `${attenDb.toFixed(0)} dB`;
  const gainValueText =
    gainDb > 0 ? `+${gainDb.toFixed(1)} dB` : `${gainDb.toFixed(1)} dB`;
  const vadValueText =
    vadThreshDb > 0
      ? `+${vadThreshDb.toFixed(1)} dB`
      : `${vadThreshDb.toFixed(1)} dB`;

  // Detect configured devices that are temporarily unplugged so <select> never falsely displays
  // system default while the backend is waiting to auto-reconnect the configured hardware.
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
  const configuredAecRefMissing = Boolean(
    config?.aec_reference_device &&
      devices &&
      !physicalOutputs.some((d) => d.name === config.aec_reference_device)
  );

  return (
    <div className="app-shell">
      <div className="topbar-sticky">
        <header
          className={`topbar ${isMac ? "is-mac" : "is-win"}`}
          data-tauri-drag-region="deep"
        >
          <h1 className="visually-hidden">Shizuka</h1>
          <div className="topbar-start">
            {!isMac && (
              <ShizukaMark
                active={Boolean(telemetry?.stream_running && ncEnabled)}
                muted={micMuted}
              />
            )}
          </div>
          <div className="topbar-end">
            <div className="topbar-actions">
              <button
                type="button"
                className="topbar-icon-btn"
                onClick={handleToggleColorScheme}
              >
                <span className="theme-branch if-dark">
                  <Sun size={14} aria-hidden="true" />
                  <span className="visually-hidden">{t.topbar.switchToLight}</span>
                </span>
                <span className="theme-branch if-light">
                  <Moon size={14} aria-hidden="true" />
                  <span className="visually-hidden">{t.topbar.switchToDark}</span>
                </span>
              </button>
              <button
                type="button"
                className={`topbar-icon-btn ${rescanning ? "is-busy" : ""}`}
                disabled={rescanning}
                onClick={() => void refreshDevicesAndConfig(true)}
                aria-label={t.topbar.rescanDevices}
                title={t.topbar.rescanDevices}
              >
                <RefreshCw size={14} aria-hidden="true" />
              </button>
              <button
                type="button"
                className="topbar-icon-btn settings-icon-btn"
                onClick={openSettingsDialog}
                aria-label={
                  updateInfo?.available
                    ? t.topbar.openSettingsWithUpdate
                    : t.topbar.openSettings
                }
                title={
                  updateInfo?.available
                    ? t.topbar.openSettingsWithUpdate
                    : t.topbar.openSettings
                }
              >
                <Settings size={14} aria-hidden="true" />
                {updateInfo?.available && (
                  <span className="update-badge-dot" aria-hidden="true" />
                )}
              </button>
            </div>
            {!isMac && (
              <>
                <span
                  className="topbar-divider"
                  role="separator"
                  aria-orientation="vertical"
                />
                <div className="win-caption-controls">
                  <button
                    type="button"
                    className="win-caption-btn"
                    onClick={handleMinimizeWindow}
                    aria-label={t.topbar.minimizeWindow}
                    title={t.topbar.minimizeWindow}
                  >
                    <svg
                      width="10"
                      height="10"
                      viewBox="0 0 10 10"
                      fill="none"
                      stroke="currentColor"
                      strokeWidth="1.2"
                      aria-hidden="true"
                    >
                      <line x1="1" y1="5" x2="9" y2="5" />
                    </svg>
                  </button>
                  <button
                    type="button"
                    className="win-caption-btn is-close"
                    onClick={handleCloseWindow}
                    aria-label={t.topbar.closeWindow}
                    title={t.topbar.closeWindow}
                  >
                    <svg
                      width="10"
                      height="10"
                      viewBox="0 0 10 10"
                      fill="none"
                      stroke="currentColor"
                      strokeWidth="1.2"
                      strokeLinecap="round"
                      aria-hidden="true"
                    >
                      <line x1="1.5" y1="1.5" x2="8.5" y2="8.5" />
                      <line x1="8.5" y1="1.5" x2="1.5" y2="8.5" />
                    </svg>
                  </button>
                </div>
              </>
            )}
          </div>
        </header>
      </div>

      <main className="main-stack">
        {connState === "waiting_device" && (
          <div className="status-banner" role="status">
            <span className="status-banner-dot" aria-hidden="true" />
            <span>
              <strong>{t.banners.waitingMicPrefix}</strong>
              {telemetry?.waiting_device_name
                ? t.banners.waitingNamedMic(telemetry.waiting_device_name)
                : t.banners.waitingAnyMic}
            </span>
          </div>
        )}

        {connState === "reconnecting" && (
          <div className="status-banner" role="status">
            <span className="status-banner-dot" aria-hidden="true" />
            <span>{t.banners.reconnectingStream}</span>
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
                {t.hero.title}
              </label>
              <span
                className={`nc-state-pill ${ncEnabled ? "on" : "off"}`}
                aria-hidden="true"
              >
                <span className="nc-state-dot" />
                <span>{ncEnabled ? t.hero.stateOn : t.hero.stateBypass}</span>
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
                {micMuted ? t.hero.micMutedLabel : t.hero.micLiveLabel}
              </span>
              <HeroInlineStats />
              {config?.shortcut_toggle_mute && (
                <kbd className="inline-kbd">
                  {formatShortcutLabel(
                    config.shortcut_toggle_mute,
                    isMac,
                    t.settings.unassigned
                  )}
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
              <span>{micMuted ? t.hero.unmuteBtn : t.hero.muteBtn}</span>
            </button>
          </div>

          <HeroLiveMeters
            inputMeterId={inputMeterId}
            outputMeterId={outputMeterId}
            ncEnabled={ncEnabled}
            micMuted={micMuted}
            t={t}
          />
        </section>

        {/* Audio Device Routing */}
        <section className="card" aria-labelledby={`${uid}-routing-heading`}>
          <div className="section-header">
            <h2 id={`${uid}-routing-heading`} className="section-title">
              <Waves size={14} aria-hidden="true" />
              <span>{t.routing.sectionTitle}</span>
            </h2>
            <span
              className={`status-indicator ${
                telemetry?.virtual_out_connected ? "connected" : ""
              }`}
            >
              <span className="status-dot" aria-hidden="true" />
              <span>
                {telemetry?.virtual_out_connected
                  ? t.routing.statusVirtualActive
                  : t.routing.statusStandaloneTest}
              </span>
            </span>
          </div>

          <div className="field">
            <label htmlFor={micSelectId} className="field-label">
              {t.routing.inputMicLabel}
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
              <option value="">{t.routing.defaultMicOption}</option>
              {configuredInputMissing && config?.input_device && (
                <option value={config.input_device}>
                  {config.input_device} {t.routing.disconnectedSuffix}
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
              {t.routing.virtualOutLabel}
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
              <option value="">{t.routing.noVirtualOutOption}</option>
              {configuredOutputMissing && config?.output_device && (
                <option value={config.output_device}>
                  {config.output_device} {t.routing.disconnectedSuffix}
                </option>
              )}
              {devices?.outputs.map((d) => (
                <option key={d.name} value={d.name}>
                  {d.is_virtual_cable
                    ? t.routing.recommendedVirtualPrefix
                    : ""}
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
                <strong>{t.routing.driverMissingTitle}</strong>
                <br />
                {locale === "ja" ? (
                  isMac ? (
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
                  )
                ) : isMac ? (
                  <>
                    Install the bundled MIT-licensed virtual mic (
                    <code>Shizuka Virtual Mic</code>) in one click — no restart
                    required.
                  </>
                ) : (
                  <>
                    Automatically downloads and installs the official
                    WHQL-signed VB-CABLE driver (Donationware: vb-audio.com).
                    After setup, select <code>CABLE Output</code> as your
                    microphone in your call app.
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
                    aria-label={t.routing.driverSpinnerAria}
                  />
                ) : (
                  <Download size={14} aria-hidden="true" />
                )}
                <span>
                  {installingDriver
                    ? t.routing.driverInstallingBtn
                    : isMac
                    ? t.routing.driverInstallMacBtn
                    : t.routing.driverInstallWinBtn}
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
            <span>{t.tuning.sectionTitle}</span>
            <span className="summary-meta">
              {modelVariant === "standard"
                ? t.tuning.modelStandard
                : t.tuning.modelLowLatency}{" "}
              · {attenValueText}
            </span>
          </summary>
          <div className="disclosure-body">
            <fieldset className="segmented-fieldset">
              <legend className="fieldset-legend">
                {t.tuning.aiModelLegend}
              </legend>
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
                  <span>{t.tuning.modelStandard}</span>
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
                  <span>{t.tuning.modelLowLatency}</span>
                </label>
              </div>
            </fieldset>

            <fieldset className="segmented-fieldset">
              <legend className="fieldset-legend">
                {t.tuning.presetLegend}
              </legend>
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
                        agcEnabled,
                        aecEnabled
                      )
                    }
                  />
                  <span>{t.tuning.presetNatural}</span>
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
                        agcEnabled,
                        aecEnabled
                      )
                    }
                  />
                  <span>{t.tuning.presetBalanced}</span>
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
                        agcEnabled,
                        aecEnabled
                      )
                    }
                  />
                  <span>{t.tuning.presetMax}</span>
                </label>
              </div>
            </fieldset>

            <div className="slider-row">
              <div className="slider-top">
                <label htmlFor={attenSliderId}>{t.tuning.attenLabel}</label>
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
                    agcEnabled,
                    aecEnabled
                  )
                }
              />
            </div>

            <div className="slider-row">
              <div className="slider-top">
                <label htmlFor={gainSliderId}>{t.tuning.inputGainLabel}</label>
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
                    agcEnabled,
                    aecEnabled
                  )
                }
              />
            </div>

            {/* Voice Conditioning Toggles: Low-Cut Filter, Auto Volume & Echo Cancellation */}
            <div className="dsp-toggle-section">
              <div className="dsp-toggle-row">
                <div className="dsp-toggle-copy">
                  <label htmlFor={hpfSwitchId} className="vad-title">
                    {t.tuning.hpfTitle}
                  </label>
                  <span id={hpfHintId} className="field-hint">
                    {t.tuning.hpfHint}
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
                      agcEnabled,
                      aecEnabled
                    )
                  }
                  {...({ switch: "" } as Record<string, string>)}
                />
              </div>

              <div className="dsp-toggle-row">
                <div className="dsp-toggle-copy">
                  <div className="vad-title-group">
                    <label htmlFor={agcSwitchId} className="vad-title">
                      {t.tuning.agcTitle}
                    </label>
                    {agcEnabled && <AgcLiveBadge />}
                  </div>
                  <span id={agcHintId} className="field-hint">
                    {t.tuning.agcHint}
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
                      e.currentTarget.checked,
                      aecEnabled
                    )
                  }
                  {...({ switch: "" } as Record<string, string>)}
                />
              </div>

              <div className="aec-toggle-block">
                <div className="dsp-toggle-row">
                  <div className="dsp-toggle-copy">
                    <div className="vad-title-group">
                      <label htmlFor={aecSwitchId} className="vad-title">
                        {t.tuning.aecTitle}
                      </label>
                      {aecEnabled && (
                        <AecLiveBadge
                          liveMonitorEnabled={liveMonitorEnabled}
                          t={t}
                        />
                      )}
                    </div>
                    <span id={aecHintId} className="field-hint">
                      {t.tuning.aecHint}
                    </span>
                  </div>
                  <input
                    id={aecSwitchId}
                    name="aec_enabled"
                    type="checkbox"
                    role={supportsNativeSwitch ? undefined : "switch"}
                    className="nc-toggle compact"
                    checked={aecEnabled}
                    aria-describedby={aecHintId}
                    onChange={(e) =>
                      void handleParamChange(
                        attenDb,
                        pfBeta,
                        gainDb,
                        vadGateEnabled,
                        vadThreshDb,
                        hpfEnabled,
                        agcEnabled,
                        e.currentTarget.checked
                      )
                    }
                    {...({ switch: "" } as Record<string, string>)}
                  />
                </div>

                {aecEnabled && (
                  <div className="aec-ref-subpanel">
                    <label htmlFor={aecRefSelectId} className="field-label">
                      {t.tuning.aecSpeakerLabel}
                    </label>
                    <div className="select-with-icon">
                      <Volume2
                        size={14}
                        className="select-leading-icon"
                        aria-hidden="true"
                      />
                      <select
                        id={aecRefSelectId}
                        name="aec_reference_device"
                        className="brand-select has-leading-icon"
                        value={config?.aec_reference_device ?? ""}
                        aria-describedby={isMac ? aecRefHintId : undefined}
                        onChange={(e) =>
                          void handleAecReferenceChange(e.target.value || null)
                        }
                      >
                        <option value="">
                          {t.tuning.aecDefaultSpeakerOption}
                        </option>
                        {configuredAecRefMissing &&
                          config?.aec_reference_device && (
                            <option value={config.aec_reference_device}>
                              {config.aec_reference_device}{" "}
                              {t.routing.disconnectedSuffix}
                            </option>
                          )}
                        {physicalOutputs.map((d) => (
                          <option key={d.name} value={d.name}>
                            {d.is_default ? "★ " : ""}
                            {d.name}
                          </option>
                        ))}
                      </select>
                    </div>
                    {isMac && (
                      <span id={aecRefHintId} className="field-hint">
                        {t.tuning.aecMacPermissionHint}
                      </span>
                    )}
                  </div>
                )}
              </div>
            </div>

            <div className="vad-section">
              <div className="vad-header">
                <div className="vad-title-group">
                  <label htmlFor={vadSwitchId} className="vad-title">
                    {t.tuning.vadTitle}
                  </label>
                  <VadLiveBadge
                    vadGateEnabled={vadGateEnabled}
                    ncEnabled={ncEnabled}
                    t={t}
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
                      agcEnabled,
                      aecEnabled
                    )
                  }
                  {...({ switch: "" } as Record<string, string>)}
                />
              </div>

              <div className="slider-row">
                <div className="slider-top">
                  <label htmlFor={vadSliderId}>{t.tuning.vadSliderLabel}</label>
                  <span className="slider-val">{vadValueText}</span>
                </div>
                <span id={vadHintId} className="field-hint">
                  {t.tuning.vadSliderHint}
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
                      agcEnabled,
                      aecEnabled
                    )
                  }
                />
              </div>
            </div>

            <div className="vad-section voice-gate-section">
              <div className="dsp-toggle-row">
                <div className="dsp-toggle-copy">
                  <div className="vad-title-group">
                    <label htmlFor={voiceGateSwitchId} className="vad-title">
                      {t.tuning.voiceGateTitle}
                    </label>
                    <VoiceGateLiveBadge
                      voiceGateEnabled={voiceGateEnabled}
                      hasVoiceProfile={hasVoiceProfile}
                      ncEnabled={ncEnabled}
                      t={t}
                    />
                  </div>
                  <span id={voiceGateHintId} className="field-hint">
                    {t.tuning.voiceGateHint}
                  </span>
                </div>
                <input
                  id={voiceGateSwitchId}
                  name="voice_gate_enabled"
                  type="checkbox"
                  role={supportsNativeSwitch ? undefined : "switch"}
                  className="nc-toggle compact"
                  checked={voiceGateEnabled}
                  aria-describedby={voiceGateHintId}
                  onChange={(e) =>
                    void handleVoiceGateChange(
                      e.currentTarget.checked,
                      voiceGateThresh
                    )
                  }
                  {...({ switch: "" } as Record<string, string>)}
                />
              </div>

              {(voiceGateEnabled || hasVoiceProfile) && (
                <div className="voice-enroll-box">
                  {hasVoiceProfile ? (
                    <div className="voice-enroll-registered">
                      <div className="voice-enroll-status">
                        <CheckCircle2
                          size={13}
                          className="voice-enroll-check"
                          aria-hidden="true"
                        />
                        <span role="status">{t.tuning.voiceEnrolledStatus}</span>
                      </div>
                      <button
                        type="button"
                        className="shortcut-record-btn"
                        onClick={() => void handleResetVoiceProfile()}
                      >
                        <RefreshCw size={12} aria-hidden="true" />
                        <span>{t.tuning.voiceRelearnBtn}</span>
                      </button>
                    </div>
                  ) : (
                    <VoiceAutoLearnProgress
                      progressId={voiceEnrollProgressId}
                      t={t}
                    />
                  )}
                </div>
              )}

              {hasVoiceProfile && (
                <div className="slider-row">
                  <div className="slider-top">
                    <label htmlFor={voiceGateSliderId}>
                      {t.tuning.voiceStrictnessLabel}
                    </label>
                    <span className="slider-val">{voiceGateValueText}</span>
                  </div>
                  <span id={voiceGateSliderHintId} className="field-hint">
                    {t.tuning.voiceStrictnessHint}
                  </span>
                  <input
                    id={voiceGateSliderId}
                    name="voice_gate_threshold"
                    type="range"
                    className="range-control"
                    min={0.16}
                    max={0.4}
                    step={0.01}
                    value={voiceGateThresh}
                    disabled={!voiceGateEnabled}
                    aria-valuetext={voiceGateValueText}
                    aria-describedby={voiceGateSliderHintId}
                    style={sliderCssVars(voiceGateThresh, 0.16, 0.4)}
                    onChange={(e) =>
                      void handleVoiceGateChange(
                        voiceGateEnabled,
                        Number(e.target.value)
                      )
                    }
                  />
                </div>
              )}
            </div>
          </div>
        </details>

        {/* Secondary Disclosure: Discord-Style Real-Time Mic Test */}
        <details className="disclosure-card" open>
          <summary>
            <Headphones size={14} aria-hidden="true" />
            <span>{t.micTest.sectionTitle}</span>
            {liveMonitorEnabled && (
              <span className="test-live-pill" aria-hidden="true">
                <span className="test-live-dot" />
                <span>{t.micTest.testingBadge}</span>
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
                  liveMonitorEnabled ? t.micTest.stopAria : t.micTest.startAria
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
                {t.micTest.meterLabel}
              </label>
              <MicTestLiveMeter
                monMeterId={monMeterId}
                liveMonitorEnabled={liveMonitorEnabled}
                t={t}
              />
            </div>

            {/* Row 2: Instant Denoised / Raw Audio A/B Toggle */}
            <fieldset className="segmented-fieldset mic-test-mode">
              <legend className="visually-hidden">
                {t.micTest.modeLegend}
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
                  <span>{t.micTest.modeDenoised}</span>
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
                  <span>{t.micTest.modeRaw}</span>
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
                <span className="visually-hidden">{t.micTest.volumeLabel}</span>
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
                {t.micTest.speakerLabel}
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
                <option value="">{t.micTest.defaultSpeakerOption}</option>
                {configuredMonitorMissing && config?.monitor_device && (
                  <option value={config.monitor_device}>
                    {config.monitor_device} {t.routing.disconnectedSuffix}
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
        <span>{t.footer.closeRunsInBackground}</span>
        <button
          type="button"
          className="quit-btn"
          onClick={() => void handleQuit()}
        >
          {t.footer.quitBtn}
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
            <div className="settings-header-start">
              {settingsView === "release_notes" && (
                <button
                  ref={releaseNotesBackRef}
                  type="button"
                  className="icon-btn settings-back-btn"
                  onClick={returnToSettingsMain}
                  aria-label={t.settings.backToSettingsAria}
                >
                  <ArrowLeft size={14} aria-hidden="true" />
                </button>
              )}
              <h2 id={settingsTitleId} className="settings-dialog-title">
                {settingsView === "release_notes" ? (
                  <>
                    <FileText size={14} aria-hidden="true" />
                    <span>{t.settings.releaseNotesTitle}</span>
                  </>
                ) : (
                  <>
                    <Settings size={14} aria-hidden="true" />
                    <span>{t.settings.title}</span>
                  </>
                )}
              </h2>
            </div>
            <button
              type="button"
              className="icon-btn"
              onClick={() => void closeSettingsDialog()}
              aria-label={t.settings.closeAria}
            >
              <X size={14} aria-hidden="true" />
            </button>
          </div>

          {settingsView === "release_notes" ? (
            <div className="settings-dialog-body release-notes-body">
              {allReleaseEntries.map((entry, idx) => {
                const isAvailable = Boolean(
                  updateInfo?.available &&
                    updateInfo.latest_version === entry.version
                );
                const isCurrent =
                  !isAvailable &&
                  (displayVersion
                    ? entry.version === displayVersion
                    : idx === 0);
                const entryTitle = getReleaseTitle(entry, locale);
                const entryItems = getReleaseItems(entry, locale);

                return (
                  <details
                    key={entry.version}
                    className={`release-entry-card ${
                      isAvailable ? "is-available" : ""
                    }`}
                    open={isAvailable || isCurrent}
                  >
                    <summary className="release-entry-summary">
                      <div className="release-summary-content">
                        <div className="release-summary-top">
                          <span className="release-version-tag">
                            v{entry.version}
                          </span>
                          {isAvailable && (
                            <span className="release-version-pill is-available">
                              {t.settings.badgeAvailable}
                            </span>
                          )}
                          {isCurrent && (
                            <span className="release-version-pill is-current">
                              {t.settings.badgeCurrent}
                            </span>
                          )}
                          {entry.date && (
                            <span className="release-date">{entry.date}</span>
                          )}
                        </div>
                        {entryTitle && (
                          <span className="release-summary-subtitle">
                            {entryTitle}
                          </span>
                        )}
                      </div>
                    </summary>
                    <div className="release-entry-body">
                      <ReleaseBulletList items={entryItems} />
                    </div>
                  </details>
                );
              })}
            </div>
          ) : (
            <div className="settings-dialog-body">
            <div className="settings-group">
              <span className="settings-group-label">
                {t.settings.generalGroup}
              </span>

              <fieldset className="segmented-fieldset">
                <legend className="fieldset-legend">
                  {t.settings.languageLegend}
                </legend>
                <div className="segmented-grid">
                  <label
                    htmlFor={langAutoId}
                    className={`seg-option ${
                      languagePref === "auto" ? "is-selected" : ""
                    }`}
                  >
                    <input
                      ref={
                        languagePref === "auto" ? activeLangInputRef : undefined
                      }
                      id={langAutoId}
                      type="radio"
                      name="app_language"
                      className="visually-hidden"
                      checked={languagePref === "auto"}
                      onChange={() => void handleLanguageChange("auto")}
                    />
                    <span>{t.settings.languageAuto}</span>
                  </label>
                  <label
                    htmlFor={langJaId}
                    className={`seg-option ${
                      languagePref === "ja" ? "is-selected" : ""
                    }`}
                  >
                    <input
                      ref={
                        languagePref === "ja" ? activeLangInputRef : undefined
                      }
                      id={langJaId}
                      type="radio"
                      name="app_language"
                      className="visually-hidden"
                      checked={languagePref === "ja"}
                      onChange={() => void handleLanguageChange("ja")}
                    />
                    <span lang="ja">日本語</span>
                  </label>
                  <label
                    htmlFor={langEnId}
                    className={`seg-option ${
                      languagePref === "en" ? "is-selected" : ""
                    }`}
                  >
                    <input
                      ref={
                        languagePref === "en" ? activeLangInputRef : undefined
                      }
                      id={langEnId}
                      type="radio"
                      name="app_language"
                      className="visually-hidden"
                      checked={languagePref === "en"}
                      onChange={() => void handleLanguageChange("en")}
                    />
                    <span lang="en">English</span>
                  </label>
                </div>
              </fieldset>

              <div className="settings-toggle-row">
                <div className="settings-toggle-copy">
                  <label
                    htmlFor={autostartSwitchId}
                    className="settings-toggle-title"
                  >
                    {t.settings.autostartTitle}
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
                {t.settings.shortcutsGroup}
              </span>

              <div className="shortcut-row">
                <div className="shortcut-copy">
                  <span className="shortcut-title">
                    {t.settings.shortcutToggleNc}
                  </span>
                  <kbd
                    className={`shortcut-kbd ${
                      config?.shortcut_toggle_nc ? "" : "is-empty"
                    }`}
                  >
                    {formatShortcutLabel(
                      config?.shortcut_toggle_nc,
                      isMac,
                      t.settings.unassigned
                    )}
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
                        ? t.settings.recordingPrompt
                        : t.settings.recordBtn}
                    </span>
                    <span className="visually-hidden">
                      {t.settings.recordNcContext}
                    </span>
                  </button>
                  {config?.shortcut_toggle_nc && (
                    <button
                      type="button"
                      className="icon-btn shortcut-clear-btn"
                      onClick={() => void handleClearShortcut("toggle_nc")}
                      aria-label={t.settings.clearNcAria}
                    >
                      <X size={12} aria-hidden="true" />
                    </button>
                  )}
                </div>
              </div>

              <div className="shortcut-row">
                <div className="shortcut-copy">
                  <span className="shortcut-title">
                    {t.settings.shortcutToggleMute}
                  </span>
                  <kbd
                    className={`shortcut-kbd ${
                      config?.shortcut_toggle_mute ? "" : "is-empty"
                    }`}
                  >
                    {formatShortcutLabel(
                      config?.shortcut_toggle_mute,
                      isMac,
                      t.settings.unassigned
                    )}
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
                        ? t.settings.recordingPrompt
                        : t.settings.recordBtn}
                    </span>
                    <span className="visually-hidden">
                      {t.settings.recordMuteContext}
                    </span>
                  </button>
                  {config?.shortcut_toggle_mute && (
                    <button
                      type="button"
                      className="icon-btn shortcut-clear-btn"
                      onClick={() => void handleClearShortcut("toggle_mute")}
                      aria-label={t.settings.clearMuteAria}
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
                    {t.settings.shortcutSoundTitle}
                  </label>
                  <span id={cueSoundHintId} className="field-hint">
                    {t.settings.shortcutSoundHint(isMac)}
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
              <span className="settings-group-label">
                {t.settings.diagnosticsGroup}
              </span>
              <SettingsDiagGrid connState={connState} t={t} />
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
                  {diagCopied
                    ? t.settings.diagCopiedBtn
                    : t.settings.diagCopyBtn}
                </span>
              </button>
              {diagError && (
                <div className="error-banner" role="alert">
                  {diagError}
                </div>
              )}
            </div>

            <div className="settings-group">
              <span className="settings-group-label">
                {t.settings.updatesGroup}
              </span>
              <div className="update-card">
                <div className="update-card-top">
                  <div className="update-copy">
                    <span className="update-title">
                      {displayVersion ? `Shizuka v${displayVersion}` : "Shizuka"}
                    </span>
                    <span className="field-hint" role="status">
                      {updateChecking
                        ? t.settings.updateChecking
                        : updateInfo?.available && updateInfo.latest_version
                        ? t.settings.updateAvailable(
                            updateInfo.latest_version,
                            updateInfo.is_portable
                          )
                        : updateInfo
                        ? t.settings.updateUpToDate
                        : t.settings.updateCheckPrompt}
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
                        aria-label={t.settings.updateCheckingSpinnerAria}
                      />
                    ) : (
                      <RefreshCw size={12} aria-hidden="true" />
                    )}
                    <span>{t.settings.updateCheckBtn}</span>
                  </button>
                </div>

                {updateInfo?.available && (
                  <div className="update-action-box">
                    {pendingUpdateItems.length > 0 && (
                      <div className="update-notes-preview">
                        <span className="update-notes-heading">
                          {t.settings.whatsNewHeading(
                            updateInfo.latest_version ?? ""
                          )}
                        </span>
                        <ReleaseBulletList items={pendingUpdateItems} compact />
                      </div>
                    )}

                    {updateInstalling && (
                      <div className="update-progress-wrap">
                        <div className="slider-top">
                          <label htmlFor={updateProgressId}>
                            {updateProgress.phase === "installing"
                              ? t.settings.updateInstallingProgress
                              : t.settings.updateDownloadingProgress}
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
                          {t.settings.updatePortableHint}
                        </span>
                        <button
                          type="button"
                          className="btn primary"
                          onClick={() => void handleOpenReleasesPage()}
                        >
                          <ExternalLink size={14} aria-hidden="true" />
                          <span>{t.settings.openReleasesBtn}</span>
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
                            ? t.settings.updateApplyingBtn
                            : t.settings.updateNowBtn(
                                updateInfo.latest_version ?? ""
                              )}
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

                <div className="update-links-row">
                  <button
                    ref={releaseNotesTriggerRef}
                    type="button"
                    className="shortcut-record-btn update-link-btn"
                    onClick={openReleaseNotesView}
                  >
                    <FileText size={12} aria-hidden="true" />
                    <span>{t.settings.releaseNotesBtn}</span>
                  </button>
                  {!(updateInfo?.available && updateInfo.is_portable) && (
                    <button
                      type="button"
                      className="shortcut-record-btn update-link-btn"
                      onClick={() => void handleOpenReleasesPage()}
                      aria-label={t.settings.openGithubReleasesAria}
                    >
                      <ExternalLink size={12} aria-hidden="true" />
                      <span>{t.settings.githubReleasesBtn}</span>
                    </button>
                  )}
                </div>
              </div>
            </div>
          </div>
          )}

          <div className="settings-dialog-footer">
            <span>
              {displayVersion ? `Shizuka v${displayVersion}` : "Shizuka"}
            </span>
            {settingsView === "release_notes" && (
              <button
                type="button"
                className="footer-link-btn"
                onClick={() => void handleOpenReleasesPage()}
                aria-label={t.settings.openGithubReleasesAria}
              >
                <span>{t.settings.githubReleasesBtn}</span>
                <ExternalLink size={11} aria-hidden="true" />
              </button>
            )}
          </div>
        </div>
      </dialog>
    </div>
  );
}
