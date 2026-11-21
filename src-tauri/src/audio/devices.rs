use cpal::traits::{DeviceTrait, HostTrait};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioDeviceInfo {
    pub name: String,
    pub is_default: bool,
    pub is_virtual_cable: bool,
    pub sample_rate: u32,
    pub channels: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceListResponse {
    pub inputs: Vec<AudioDeviceInfo>,
    pub outputs: Vec<AudioDeviceInfo>,
    pub recommended_virtual_output: Option<String>,
    pub has_virtual_cable: bool,
    pub platform: String,
}

/// Checks whether a device name corresponds to a known virtual audio cable
/// (VB-CABLE / VoiceMeeter on Windows, BlackHole / Loopback / Soundflower on macOS).
pub fn is_virtual_audio_device(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.contains("shizuka virtual")
        || lower.contains("clearvoice virtual")
        || lower.contains("cable input")
        || lower.contains("cable output")
        || lower.contains("vb-audio")
        || lower.contains("vb-cable")
        || lower.contains("blackhole")
        || lower.contains("soundflower")
        || lower.contains("voicemeeter")
        || lower.contains("virtual audio")
        || lower.contains("loopback audio")
}

/// Higher score means better candidate for the virtual microphone output sink.
fn virtual_output_priority(name: &str) -> u8 {
    let lower = name.to_lowercase();
    if lower.contains("shizuka virtual") {
        115 // Native MIT Shizuka HAL driver on macOS
    } else if lower.contains("clearvoice virtual") {
        110 // Legacy ClearVoice HAL driver compatibility on macOS
    } else if lower.contains("cable input") || lower.contains("blackhole 2ch") {
        100 // VB-CABLE primary sink on Windows / BlackHole 2ch primary sink on macOS
    } else if lower.contains("blackhole") || lower.contains("vb-audio") {
        85
    } else if lower.contains("voicemeeter") || lower.contains("virtual audio") {
        70
    } else {
        0
    }
}

/// Filters out multi-channel auxiliary virtual cable endpoints (such as VB-CABLE's "CABLE In 16ch")
/// that are unnecessary and confusing for voice microphone routing.
fn should_hide_device(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.contains("cable in 16ch") || lower.contains("cable out 16ch")
}

pub fn list_audio_devices() -> DeviceListResponse {
    let host = cpal::default_host();

    let default_in_name = host
        .default_input_device()
        .and_then(|d| d.name().ok());
    let default_out_name = host
        .default_output_device()
        .and_then(|d| d.name().ok());

    let mut inputs = Vec::new();
    if let Ok(devs) = host.input_devices() {
        for dev in devs {
            if let Ok(name) = dev.name() {
                if should_hide_device(&name) {
                    continue;
                }
                // Skip virtual cable outputs in physical microphone list? Keep them marked so user sees clearly
                let (sr, ch) = dev
                    .default_input_config()
                    .map(|c| (c.sample_rate().0, c.channels()))
                    .unwrap_or((48_000, 1));
                let is_def = default_in_name.as_deref() == Some(name.as_str());
                let is_virt = is_virtual_audio_device(&name);
                inputs.push(AudioDeviceInfo {
                    name,
                    is_default: is_def,
                    is_virtual_cable: is_virt,
                    sample_rate: sr,
                    channels: ch,
                });
            }
        }
    }

    let mut outputs = Vec::new();
    let mut best_virtual: Option<(u8, String)> = None;

    if let Ok(devs) = host.output_devices() {
        for dev in devs {
            if let Ok(name) = dev.name() {
                if should_hide_device(&name) {
                    continue;
                }
                let (sr, ch) = dev
                    .default_output_config()
                    .map(|c| (c.sample_rate().0, c.channels()))
                    .unwrap_or((48_000, 2));
                let is_def = default_out_name.as_deref() == Some(name.as_str());
                let is_virt = is_virtual_audio_device(&name);
                let prio = virtual_output_priority(&name);
                if prio > 0 {
                    match &best_virtual {
                        Some((best_p, _)) if *best_p >= prio => {}
                        _ => best_virtual = Some((prio, name.clone())),
                    }
                }
                outputs.push(AudioDeviceInfo {
                    name,
                    is_default: is_def,
                    is_virtual_cable: is_virt,
                    sample_rate: sr,
                    channels: ch,
                });
            }
        }
    }

    // Sort virtual cables to top of outputs for convenience, while keeping physical mics at top of inputs
    inputs.sort_by_key(|d| (d.is_virtual_cable, !d.is_default));
    outputs.sort_by_key(|d| (!d.is_virtual_cable, !d.is_default));

    let recommended_virtual_output = best_virtual.map(|(_, n)| n);
    let has_virtual_cable = recommended_virtual_output.is_some();

    let platform = if cfg!(target_os = "macos") {
        "macos".to_string()
    } else if cfg!(target_os = "windows") {
        "windows".to_string()
    } else {
        std::env::consts::OS.to_string()
    };

    DeviceListResponse {
        inputs,
        outputs,
        recommended_virtual_output,
        has_virtual_cable,
        platform,
    }
}

/// Returns the current OS default physical (non-virtual) microphone name, if any.
pub fn default_physical_input_name() -> Option<String> {
    let host = cpal::default_host();
    if let Some(def) = host.default_input_device() {
        if let Ok(name) = def.name() {
            if !is_virtual_audio_device(&name) {
                return Some(name);
            }
        }
    }
    host.input_devices()
        .ok()?
        .filter_map(|d| d.name().ok())
        .find(|n| !should_hide_device(n) && !is_virtual_audio_device(n))
}

/// Finds an input device by exact name when `preferred_name` is `Some` (never silently falls back
/// to a different microphone so unplugged USB mics enter a clean waiting-for-reconnect state).
/// When `preferred_name` is `None` or empty ("auto"), selects the OS default physical microphone.
pub fn find_input_device(preferred_name: Option<&str>) -> Option<cpal::Device> {
    let host = cpal::default_host();
    if let Some(target) = preferred_name.filter(|s| !s.is_empty()) {
        if let Ok(mut devs) = host.input_devices() {
            return devs.find(|d| d.name().ok().as_deref() == Some(target));
        }
        return None;
    }
    // Auto mode: prefer non-virtual OS default input device
    if let Some(def) = host.default_input_device() {
        if let Ok(name) = def.name() {
            if !is_virtual_audio_device(&name) {
                return Some(def);
            }
        } else {
            return Some(def);
        }
    }
    host.input_devices().ok()?.find(|d| {
        d.name()
            .map(|n| !should_hide_device(&n) && !is_virtual_audio_device(&n))
            .unwrap_or(true)
    })
}

pub fn find_output_device(preferred_name: Option<&str>, fallback_to_default: bool) -> Option<cpal::Device> {
    let host = cpal::default_host();
    if let Some(target) = preferred_name {
        if !target.is_empty() {
            if let Ok(mut devs) = host.output_devices() {
                if let Some(found) = devs.find(|d| d.name().ok().as_deref() == Some(target)) {
                    return Some(found);
                }
            }
        }
    }
    if fallback_to_default {
        // Prefer a non-virtual physical speaker when used as monitor fallback
        if let Some(def) = host.default_output_device() {
            if let Ok(name) = def.name() {
                if !is_virtual_audio_device(&name) {
                    return Some(def);
                }
            } else {
                return Some(def);
            }
        }
        host.output_devices()
            .ok()?
            .find(|d| d.name().map(|n| !is_virtual_audio_device(&n)).unwrap_or(true))
    } else {
        None
    }
}
