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

export interface AppConfig {
  input_device: string | null;
  output_device: string | null;
  monitor_device: string | null;
  nc_enabled: boolean;
  model_variant: ModelVariant;
  atten_lim_db: number;
  post_filter_beta: number;
  input_gain_db: number;
  min_db_thresh: number;
  vad_gate_enabled: boolean;
  vad_threshold_db: number;
  monitor_volume: number;
  stream_active: boolean;
}

export interface EngineDeviceStatus {
  active_input_name: string | null;
  active_output_name: string | null;
  active_monitor_name: string | null;
  input_sample_rate: number;
  output_sample_rate: number;
  last_error: string | null;
}

export interface AudioTelemetry {
  stream_running: boolean;
  nc_enabled: boolean;
  model_variant: ModelVariant;
  vad_gate_enabled: boolean;
  vad_speaking: boolean;
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
