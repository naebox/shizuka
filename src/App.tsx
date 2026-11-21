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
  Check,
  CheckCircle2,
  ClipboardCopy,
  Download,
  Headphones,
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
} from "./types";

const supportsNativeSwitch =
  typeof HTMLInputElement !== "undefined" &&
  "switch" in HTMLInputElement.prototype;

const COLOR_SCHEME_STORAGE_KEY = "color-scheme";

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
  const nativeTheme = content === "dark" || content === "light" ? content : null;
  void invoke("set_window_theme", { theme: nativeTheme }).catch(() => {});
}

function ShizukaMark({ active }: { active: boolean }) {
  return (
    <svg
      className={`brand-mark ${active ? "active" : ""}`}
      viewBox="0 0 100 100"
      aria-hidden="true"
    >
      <rect
        className="mark-tile"
        x="4"
        y="4"
        width="92"
        height="92"
        rx="24"
        strokeWidth="3"
      />
      <path
        className="mark-stroke"
        d="M 61.8 81.4 A 33.5 33.5 0 1 1 81.4 61.8"
        fill="none"
        strokeWidth="7.4"
        strokeLinecap="round"
      />
      <circle className="mark-led" cx="73.7" cy="73.7" r="6.2" />
      <rect className="mark-bars" x="32.2" y="39" width="8.2" height="22" rx="4.1" />
      <rect className="mark-bars" x="45.9" y="31" width="8.2" height="38" rx="4.1" />
      <rect className="mark-bars" x="59.6" y="39" width="8.2" height="22" rx="4.1" />
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

  const settingsDialogRef = useRef<HTMLDialogElement | null>(null);
  const lastReconnectCountRef = useRef<number>(0);

  const [devices, setDevices] = useState<DeviceListResponse | null>(null);
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [telemetry, setTelemetry] = useState<AudioTelemetry | null>(null);
  const [monitorClean, setMonitorClean] = useState(true);
  const [autostartEnabled, setAutostartEnabled] = useState(false);
  const [autostartBusy, setAutostartBusy] = useState(false);
  const [autostartError, setAutostartError] = useState<string | null>(null);
  const [diagCopied, setDiagCopied] = useState(false);
  const [installingDriver, setInstallingDriver] = useState(false);
  const [installMessage, setInstallMessage] = useState<{
    ok: boolean;
    text: string;
  } | null>(null);

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

  const closeSettingsDialog = useCallback(async () => {
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
  }, []);

  const openSettingsDialog = useCallback(() => {
    const dialog = settingsDialogRef.current;
    if (!dialog || dialog.open) return;
    dialog.removeAttribute("data-closing");
    setAutostartError(null);
    setDiagCopied(false);
    void invoke<boolean>("get_autostart_enabled")
      .then(setAutostartEnabled)
      .catch(() => {});
    dialog.showModal();
  }, []);

  useEffect(() => {
    const dialog = settingsDialogRef.current;
    if (!dialog) return;

    const onClose = () => {
      dialog.removeAttribute("data-closing");
      dialog.returnValue = "";
    };
    dialog.addEventListener("close", onClose);

    const supportsNativeTopLayerExit =
      typeof window !== "undefined" &&
      Boolean(window.CSS?.supports?.("overlay", "auto"));
    const onCancel = (e: Event) => {
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
      if (pressedOutside && isOutside(e)) {
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
  }, [closeSettingsDialog]);

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
        }
      } catch {
        // Ignore transient IPC error during shutdown
      }
    }, 50);
    return () => clearInterval(timer);
  }, [refreshDevicesAndConfig]);

  const ncEnabled = telemetry?.nc_enabled ?? config?.nc_enabled ?? true;

  const handleToggleNc = async (next: boolean) => {
    setConfig((prev) => (prev ? { ...prev, nc_enabled: next } : prev));
    await invoke("set_nc_enabled", { enabled: next });
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
    const updated: AppConfig = {
      ...config,
      input_device: nextIn,
      output_device: nextOut,
      monitor_device: nextMon,
    };
    setConfig(updated);
    await invoke("select_devices", {
      inputDevice: nextIn,
      outputDevice: nextOut,
      monitorDevice: nextMon,
    });
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
    vadThresholdDb: number
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
    });
    await invoke("update_dsp_params", {
      attenLimDb,
      postFilterBeta,
      inputGainDb,
      minDbThresh: minThresh,
      vadGateEnabled,
      vadThresholdDb,
    });
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
  const pfBeta = config?.post_filter_beta ?? 0.02;
  const gainDb = config?.input_gain_db ?? 0;
  const vadGateEnabled =
    config?.vad_gate_enabled ?? telemetry?.vad_gate_enabled ?? true;
  const vadThreshDb = config?.vad_threshold_db ?? -5;
  const vadSpeaking = telemetry?.vad_speaking ?? false;
  const liveMonitorEnabled = telemetry?.live_monitor_enabled ?? false;
  const monVolPct = Math.round(
    (config?.monitor_volume ?? telemetry?.monitor_volume ?? 0.8) * 100
  );
  const monLevelRatio = liveMonitorEnabled
    ? dbToRatio(telemetry?.monitor_rms_db ?? -90)
    : 0;
  const isMac = devices?.platform === "macos";
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

  return (
    <div className="app-shell">
      <header className="topbar">
        <div className="brand">
          <ShizukaMark active={Boolean(telemetry?.stream_running && ncEnabled)} />
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
              style={{ verticalAlign: "text-bottom", marginInlineEnd: "0.3rem" }}
            />
            {telemetry.status.last_error}
          </div>
        )}

        {/* Primary Noise Cancellation Switch & Live Meters */}
        <section
          className="card hero-card"
          aria-labelledby={`${uid}-hero-heading`}
        >
          <div className="hero-top">
            <div className="hero-title-row">
              <span className="hero-icon-box" aria-hidden="true">
                {ncEnabled ? <Mic size={15} /> : <MicOff size={15} />}
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
                <span>{ncEnabled ? "ON" : "OFF"}</span>
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
                    {ncEnabled ? "ノイズ除去後" : "スルー出力 (未処理)"}
                  </label>
                  {ncEnabled && (telemetry?.noise_reduced_db ?? 0) > 1.5 && (
                    <span className="reduction-badge">
                      -{(telemetry?.noise_reduced_db ?? 0).toFixed(0)} dB
                    </span>
                  )}
                </span>
                <span>{(telemetry?.output_rms_db ?? -90).toFixed(0)} dB</span>
              </div>
              <progress
                id={outputMeterId}
                className={`audio-meter ${ncEnabled ? "" : "raw"}`}
                max={1}
                value={dbToRatio(telemetry?.output_rms_db ?? -90)}
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

        {/* Secondary Disclosure: Noise Suppression Strength, AI Model & VAD Gate */}
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
                        vadThreshDb
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
                        0.02,
                        gainDb,
                        vadGateEnabled,
                        vadThreshDb
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
                        0.035,
                        gainDb,
                        vadGateEnabled,
                        vadThreshDb
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
                    vadThreshDb
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
                    vadThreshDb
                  )
                }
              />
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
                      vadThreshDb
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
                      Number(e.target.value)
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
        <span>
          クロック同期: {(telemetry?.drift_ppm ?? 0) >= 0 ? "+" : ""}
          {(telemetry?.drift_ppm ?? 0).toFixed(0)} ppm | 閉じる[×]でトレイ常駐
        </span>
        <button
          type="button"
          className="quit-btn"
          onClick={() => void handleQuit()}
        >
          完全終了
        </button>
      </footer>

      {/* Native Modal Settings Dialog (<dialog> with light-dismiss & top-layer transitions) */}
      <dialog
        ref={settingsDialogRef}
        className="settings-dialog"
        aria-labelledby={settingsTitleId}
        {...({ closedby: "any" } as Record<string, string>)}
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
              <span className="settings-group-label">起動と常駐</span>
              <div className="settings-toggle-row">
                <div className="settings-toggle-copy">
                  <label
                    htmlFor={autostartSwitchId}
                    className="settings-toggle-title"
                  >
                    OS 起動時にトレイへ常駐起動
                  </label>
                  <span id={autostartHintId} className="field-hint">
                    ウィンドウを出さず{isMac ? "メニューバー" : "タスクトレイ"}で静かに待機します
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
              <span className="settings-group-label">自動復旧・診断情報</span>
              <div className="diag-grid">
                <div className="diag-item">
                  <span className="diag-key">パイプライン状態</span>
                  <span className="diag-val">{connStateLabel}</span>
                </div>
                <div className="diag-item">
                  <span className="diag-key">自動復旧回数</span>
                  <span className="diag-val">
                    {telemetry?.status.reconnect_count ?? 0} 回
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
            <span>Shizuka v0.2.0</span>
            <span>ESC または外側クリックで閉じる</span>
          </div>
        </div>
      </dialog>
    </div>
  );
}
