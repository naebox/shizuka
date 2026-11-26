import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AudioTelemetry, ConnectionState, ModelVariant } from "./types";

/**
 * Low-frequency discrete subset of `AudioTelemetry`.
 * Subscribing to this instead of full telemetry prevents the root `App` tree
 * from re-rendering 20x/sec when only audio meters / CPU / latency change.
 */
export interface DiscreteTelemetry {
  stream_running: boolean;
  nc_enabled: boolean;
  mic_muted: boolean;
  model_variant: ModelVariant;
  vad_gate_enabled: boolean;
  hpf_enabled: boolean;
  agc_enabled: boolean;
  aec_enabled: boolean;
  aec_active: boolean;
  voice_gate_enabled: boolean;
  has_voice_profile: boolean;
  config_revision: number;
  virtual_out_connected: boolean;
  live_monitor_enabled: boolean;
  monitor_volume: number;
  connection_state: ConnectionState;
  waiting_device_name: string | null;
  reconnect_count: number;
  last_error: string | null;
}

const POLL_INTERVAL_MS = 50;

let latestRaw: AudioTelemetry | null = null;
let latestDiscrete: DiscreteTelemetry | null = null;

const rawListeners = new Set<() => void>();
const discreteListeners = new Set<() => void>();

let timerId: number | null = null;
let inFlight = false;
let nativeVisible = false;
let domVisible =
  typeof document === "undefined" ? true : document.visibilityState !== "hidden";
let visibilityHooked = false;

function toDiscrete(snap: AudioTelemetry): DiscreteTelemetry {
  return {
    stream_running: snap.stream_running,
    nc_enabled: snap.nc_enabled,
    mic_muted: snap.mic_muted,
    model_variant: snap.model_variant,
    vad_gate_enabled: snap.vad_gate_enabled,
    hpf_enabled: snap.hpf_enabled,
    agc_enabled: snap.agc_enabled,
    aec_enabled: snap.aec_enabled,
    aec_active: snap.aec_active,
    voice_gate_enabled: snap.voice_gate_enabled,
    has_voice_profile: snap.has_voice_profile,
    config_revision: snap.config_revision,
    virtual_out_connected: snap.virtual_out_connected,
    live_monitor_enabled: snap.live_monitor_enabled,
    monitor_volume: snap.monitor_volume,
    connection_state: snap.status.connection_state,
    waiting_device_name: snap.status.waiting_device_name,
    reconnect_count: snap.status.reconnect_count,
    last_error: snap.status.last_error,
  };
}

function discreteEqual(
  a: DiscreteTelemetry | null,
  b: DiscreteTelemetry
): boolean {
  if (!a) return false;
  return (
    a.stream_running === b.stream_running &&
    a.nc_enabled === b.nc_enabled &&
    a.mic_muted === b.mic_muted &&
    a.model_variant === b.model_variant &&
    a.vad_gate_enabled === b.vad_gate_enabled &&
    a.hpf_enabled === b.hpf_enabled &&
    a.agc_enabled === b.agc_enabled &&
    a.aec_enabled === b.aec_enabled &&
    a.aec_active === b.aec_active &&
    a.voice_gate_enabled === b.voice_gate_enabled &&
    a.has_voice_profile === b.has_voice_profile &&
    a.config_revision === b.config_revision &&
    a.virtual_out_connected === b.virtual_out_connected &&
    a.live_monitor_enabled === b.live_monitor_enabled &&
    a.monitor_volume === b.monitor_volume &&
    a.connection_state === b.connection_state &&
    a.waiting_device_name === b.waiting_device_name &&
    a.reconnect_count === b.reconnect_count &&
    a.last_error === b.last_error
  );
}

function publishTelemetry(snap: AudioTelemetry) {
  latestRaw = snap;
  for (const l of rawListeners) {
    l();
  }
  const nextDiscrete = toDiscrete(snap);
  if (!discreteEqual(latestDiscrete, nextDiscrete)) {
    latestDiscrete = nextDiscrete;
    for (const l of discreteListeners) {
      l();
    }
  }
}

function shouldPoll(): boolean {
  return (
    nativeVisible &&
    domVisible &&
    (rawListeners.size > 0 || discreteListeners.size > 0)
  );
}

function stopTimer() {
  if (timerId !== null) {
    window.clearTimeout(timerId);
    timerId = null;
  }
}

function scheduleNextTick() {
  if (!shouldPoll() || timerId !== null) return;
  timerId = window.setTimeout(() => {
    timerId = null;
    void pollOnce(true);
  }, POLL_INTERVAL_MS);
}

async function pollOnce(reschedule: boolean) {
  if (inFlight) return;
  inFlight = true;
  try {
    const snap = await invoke<AudioTelemetry>("get_telemetry");
    publishTelemetry(snap);
  } catch {
    // Ignore transient IPC error during shutdown
  } finally {
    inFlight = false;
    if (reschedule && shouldPoll()) {
      scheduleNextTick();
    }
  }
}

function ensureDomVisibilityHook() {
  if (visibilityHooked || typeof document === "undefined") return;
  visibilityHooked = true;
  document.addEventListener("visibilitychange", () => {
    domVisible = document.visibilityState !== "hidden";
    if (shouldPoll()) {
      stopTimer();
      void pollOnce(true);
    } else {
      stopTimer();
    }
  });
}

export function seedTelemetry(snap: AudioTelemetry) {
  publishTelemetry(snap);
}

export function setNativeWindowVisible(visible: boolean) {
  ensureDomVisibilityHook();
  nativeVisible = visible;
  if (shouldPoll()) {
    stopTimer();
    void pollOnce(true);
  } else {
    stopTimer();
  }
}

export function refreshTelemetryNow() {
  if (!nativeVisible || !domVisible) return;
  stopTimer();
  void pollOnce(true);
}

function subscribeRaw(listener: () => void): () => void {
  ensureDomVisibilityHook();
  rawListeners.add(listener);
  if (shouldPoll() && timerId === null && !inFlight) {
    scheduleNextTick();
  }
  return () => {
    rawListeners.delete(listener);
    if (!shouldPoll()) {
      stopTimer();
    }
  };
}

function subscribeDiscrete(listener: () => void): () => void {
  ensureDomVisibilityHook();
  discreteListeners.add(listener);
  if (shouldPoll() && timerId === null && !inFlight) {
    scheduleNextTick();
  }
  return () => {
    discreteListeners.delete(listener);
    if (!shouldPoll()) {
      stopTimer();
    }
  };
}

export function useLiveTelemetry(): AudioTelemetry | null {
  return useSyncExternalStore(
    subscribeRaw,
    () => latestRaw,
    () => null
  );
}

export function useDiscreteTelemetry(): DiscreteTelemetry | null {
  return useSyncExternalStore(
    subscribeDiscrete,
    () => latestDiscrete,
    () => null
  );
}
