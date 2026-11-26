export interface AudioDeviceInfo {
  name: string;
  is_default: boolean;
  is_virtual_cable: boolean;
  sample_rate: number;
  channels: number;
}

export interface DeviceListResponse {
  inputs: AudioDeviceInfo[];
  outputs: AudioDeviceInfo[];
  recommended_virtual_output: string | null;
  has_virtual_cable: boolean;
  platform: string;
}

export type ModelVariant = "standard" | "low_latency";

export type ShortcutAction = "toggle_nc" | "toggle_mute";

export type LanguagePreference = "auto" | "ja" | "en";

export type Locale = "ja" | "en";

export interface AppConfig {
  input_device: string | null;
  output_device: string | null;
  monitor_device: string | null;
  aec_reference_device: string | null;
  nc_enabled: boolean;
  model_variant: ModelVariant;
  atten_lim_db: number;
  post_filter_beta: number;
  input_gain_db: number;
  min_db_thresh: number;
  vad_gate_enabled: boolean;
  vad_threshold_db: number;
  hpf_enabled: boolean;
  agc_enabled: boolean;
  aec_enabled: boolean;
  voice_gate_enabled: boolean;
  voice_gate_threshold: number;
  voice_profile: number[] | null;
  voice_fbank_tilt?: number[] | null;
  monitor_volume: number;
  stream_active: boolean;
  shortcut_toggle_nc: string | null;
  shortcut_toggle_mute: string | null;
  shortcut_sound_enabled: boolean;
  language: LanguagePreference;
}

export type ConnectionState =
  | "running"
  | "waiting_device"
  | "reconnecting"
  | "stopped";

export interface EngineDeviceStatus {
  connection_state: ConnectionState;
  waiting_device_name: string | null;
  active_input_name: string | null;
  active_output_name: string | null;
  active_monitor_name: string | null;
  active_aec_ref_name: string | null;
  input_sample_rate: number;
  output_sample_rate: number;
  reconnect_count: number;
  input_drop_count: number;
  output_underrun_count: number;
  last_error: string | null;
}

export interface AudioTelemetry {
  stream_running: boolean;
  nc_enabled: boolean;
  mic_muted: boolean;
  model_variant: ModelVariant;
  vad_gate_enabled: boolean;
  vad_speaking: boolean;
  hpf_enabled: boolean;
  agc_enabled: boolean;
  agc_gain_db: number;
  aec_enabled: boolean;
  aec_active: boolean;
  aec_erle_db: number;
  voice_gate_enabled: boolean;
  has_voice_profile: boolean;
  voice_gate_blocking: boolean;
  voice_gate_similarity: number;
  voice_learn_progress: number;
  clipping_active: boolean;
  clip_count: number;
  config_revision: number;
  virtual_out_connected: boolean;
  live_monitor_enabled: boolean;
  monitor_clean: boolean;
  monitor_volume: number;
  monitor_rms_db: number;
  input_rms_db: number;
  output_rms_db: number;
  noise_reduced_db: number;
  lsnr_db: number;
  drift_ppm: number;
  latency_ms: number;
  cpu_load_pct: number;
  status: EngineDeviceStatus;
}

export interface InstallDriverResult {
  success: boolean;
  device_detected_now: boolean;
  detected_device_name: string | null;
  reboot_recommended: boolean;
  message: string;
}

export interface UpdateCheckResult {
  current_version: string;
  available: boolean;
  latest_version: string | null;
  date: string | null;
  notes: string | null;
  is_portable: boolean;
  releases_url: string;
}

export interface UpdateProgressSnapshot {
  phase: "idle" | "downloading" | "installing" | "error";
  downloaded_bytes: number;
  total_bytes: number | null;
  error: string | null;
}
