use std::sync::atomic::AtomicU32;
#[cfg(not(target_os = "macos"))]
use std::sync::atomic::Ordering;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use cpal::traits::DeviceTrait;
#[cfg(not(target_os = "macos"))]
use cpal::traits::StreamTrait;
#[cfg(not(target_os = "macos"))]
use cpal::{FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig};
#[cfg(not(target_os = "macos"))]
use ringbuf::traits::{Producer, Split};
#[cfg(not(target_os = "macos"))]
use ringbuf::HeapRb;

use crate::audio::aec::AecRefSink;
use crate::audio::devices::{device_name, find_aec_reference_device};
use crate::audio::engine::StreamErrorNotifier;

/// Active OS speaker loopback stream handle.
/// Pauses and tears down the underlying OS loopback stream (WASAPI Loopback on Windows,
/// CoreAudio Process Tap + private Aggregate Device on macOS) cleanly when dropped.
pub struct LoopbackPipeline {
    #[cfg(not(target_os = "macos"))]
    _stream: Stream,
    #[cfg(target_os = "macos")]
    _macos_tap: macos_tap::MacosProcessTapStream,
}

#[cfg(not(target_os = "macos"))]
impl Drop for LoopbackPipeline {
    fn drop(&mut self) {
        let _ = self._stream.pause();
    }
}

/// Opens a non-intrusive loopback capture stream on the target physical speaker output device
/// so Shizuka's AEC3 engine receives the far-end conference speaker audio in real time.
pub(crate) fn build_aec_loopback_stream(
    ref_name: Option<&str>,
    err_notifier: StreamErrorNotifier,
) -> Result<(LoopbackPipeline, AecRefSink, String)> {
    let explicit = ref_name.filter(|s| !s.is_empty());
    let out_dev = find_aec_reference_device(explicit).ok_or_else(|| {
        if let Some(name) = explicit {
            anyhow!(
                "エコー除去用のスピーカー「{}」が見つかりません",
                name
            )
        } else {
            anyhow!("エコー除去用のスピーカーが見つかりません")
        }
    })?;

    let actual_name = device_name(&out_dev).unwrap_or_else(|| "Default Speaker".to_string());

    #[cfg(not(target_os = "macos"))]
    {
        let supported = out_dev.default_output_config().with_context(|| {
            format!(
                "エコー除去用のスピーカー「{}」のフォーマット取得に失敗しました",
                actual_name
            )
        })?;
        let sample_format = supported.sample_format();
        let mut cfg: StreamConfig = supported.into();
        cfg.buffer_size = cpal::BufferSize::Default;
        let sr_ref = cfg.sample_rate;
        let ch_ref = cfg.channels as usize;

        let rb_cap = (sr_ref as usize / 2).max(8_192);
        let rb = HeapRb::<f32>::new(rb_cap);
        let (prod, cons) = rb.split();
        let observed_block_frames = Arc::new(AtomicU32::new(0));

        let sink = AecRefSink::new(cons, sr_ref, Arc::clone(&observed_block_frames))?;
        let stream = build_cpal_loopback_to_rb(
            &out_dev,
            &cfg,
            sample_format,
            ch_ref,
            prod,
            observed_block_frames,
            err_notifier,
        )
        .with_context(|| {
            format!(
                "エコー除去用のスピーカー「{}」の音声取得に失敗しました",
                actual_name
            )
        })?;
        stream.play().with_context(|| {
            format!(
                "エコー除去用のスピーカー「{}」の音声取得開始に失敗しました",
                actual_name
            )
        })?;

        Ok((LoopbackPipeline { _stream: stream }, sink, actual_name))
    }

    #[cfg(target_os = "macos")]
    {
        let supported = out_dev.default_output_config().with_context(|| {
            format!(
                "エコー除去用のスピーカー「{}」のフォーマット取得に失敗しました",
                actual_name
            )
        })?;
        let fallback_sr = supported.sample_rate();
        let device_uid = out_dev.id().ok().map(|id| id.1);

        let observed_block_frames = Arc::new(AtomicU32::new(0));
        let (tap_stream, cons, actual_sr) = macos_tap::MacosProcessTapStream::open(
            device_uid.as_deref(),
            fallback_sr,
            Arc::clone(&observed_block_frames),
            err_notifier,
        )
        .with_context(|| {
            format!(
                "エコー除去用のシステム音声取得「{}」に失敗しました（システム設定 > プライバシーとセキュリティ > 画面収録とシステムオーディオ録音 の権限を確認してください）",
                actual_name
            )
        })?;

        let sink = AecRefSink::new(cons, actual_sr, observed_block_frames)?;
        Ok((
            LoopbackPipeline {
                _macos_tap: tap_stream,
            },
            sink,
            actual_name,
        ))
    }
}

#[cfg(not(target_os = "macos"))]
fn build_cpal_loopback_to_rb(
    device: &cpal::Device,
    config: &StreamConfig,
    format: SampleFormat,
    channels: usize,
    prod: ringbuf::HeapProd<f32>,
    observed_block_frames: Arc<AtomicU32>,
    err_notifier: StreamErrorNotifier,
) -> Result<Stream> {
    match format {
        SampleFormat::F32 => build_loopback_typed::<f32>(
            device,
            config,
            channels,
            prod,
            observed_block_frames,
            err_notifier,
        ),
        SampleFormat::F64 => build_loopback_typed::<f64>(
            device,
            config,
            channels,
            prod,
            observed_block_frames,
            err_notifier,
        ),
        SampleFormat::I8 => build_loopback_typed::<i8>(
            device,
            config,
            channels,
            prod,
            observed_block_frames,
            err_notifier,
        ),
        SampleFormat::U8 => build_loopback_typed::<u8>(
            device,
            config,
            channels,
            prod,
            observed_block_frames,
            err_notifier,
        ),
        SampleFormat::I16 => build_loopback_typed::<i16>(
            device,
            config,
            channels,
            prod,
            observed_block_frames,
            err_notifier,
        ),
        SampleFormat::U16 => build_loopback_typed::<u16>(
            device,
            config,
            channels,
            prod,
            observed_block_frames,
            err_notifier,
        ),
        SampleFormat::I24 => build_loopback_typed::<cpal::I24>(
            device,
            config,
            channels,
            prod,
            observed_block_frames,
            err_notifier,
        ),
        SampleFormat::U24 => build_loopback_typed::<cpal::U24>(
            device,
            config,
            channels,
            prod,
            observed_block_frames,
            err_notifier,
        ),
        SampleFormat::I32 => build_loopback_typed::<i32>(
            device,
            config,
            channels,
            prod,
            observed_block_frames,
            err_notifier,
        ),
        SampleFormat::U32 => build_loopback_typed::<u32>(
            device,
            config,
            channels,
            prod,
            observed_block_frames,
            err_notifier,
        ),
        other => Err(anyhow!("未対応の参照音声サンプル形式: {:?}", other)),
    }
}

#[cfg(not(target_os = "macos"))]
fn build_loopback_typed<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    channels: usize,
    mut prod: ringbuf::HeapProd<f32>,
    observed_block_frames: Arc<AtomicU32>,
    err_notifier: StreamErrorNotifier,
) -> Result<Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let ch = channels.max(1);
    // Zero-allocation WASAPI loopback callback using a fixed stack buffer.
    // Downmixes Front-L / Front-R speaker channels equally at 0.5x without heap allocation or locks.
    let stream = device.build_input_stream(
        config,
        move |data: &[T], _| {
            let mut mono_scratch = [0.0f32; 512];
            let mut count = 0usize;
            let mut total_frames = 0u32;

            match ch {
                1 => {
                    for &s in data {
                        let v = f32::from_sample(s);
                        mono_scratch[count] = if v.is_finite() { v } else { 0.0 };
                        count += 1;
                        total_frames = total_frames.saturating_add(1);
                        if count == mono_scratch.len() {
                            let _ = prod.push_slice(&mono_scratch);
                            count = 0;
                        }
                    }
                }
                2 => {
                    for frame in data.as_chunks::<2>().0 {
                        let l = f32::from_sample(frame[0]);
                        let r = f32::from_sample(frame[1]);
                        let l = if l.is_finite() { l } else { 0.0 };
                        let r = if r.is_finite() { r } else { 0.0 };
                        mono_scratch[count] = (l + r) * 0.5;
                        count += 1;
                        total_frames = total_frames.saturating_add(1);
                        if count == mono_scratch.len() {
                            let _ = prod.push_slice(&mono_scratch);
                            count = 0;
                        }
                    }
                }
                _ => {
                    // On 5.1/7.1 surround endpoints, voice/call audio lives in Front L/R + Center (ch 0..3)
                    let active_ch = ch.min(3);
                    let inv = 1.0 / (active_ch as f32);
                    for frame in data.chunks_exact(ch) {
                        let mut sum = 0.0f32;
                        for &raw in &frame[..active_ch] {
                            let v = f32::from_sample(raw);
                            if v.is_finite() {
                                sum += v;
                            }
                        }
                        mono_scratch[count] = sum * inv;
                        count += 1;
                        total_frames = total_frames.saturating_add(1);
                        if count == mono_scratch.len() {
                            let _ = prod.push_slice(&mono_scratch);
                            count = 0;
                        }
                    }
                }
            }
            if count > 0 {
                let _ = prod.push_slice(&mono_scratch[..count]);
            }
            if total_frames > 0 {
                observed_block_frames.fetch_max(total_frames, Ordering::Relaxed);
            }
        },
        move |err| err_notifier.notify(err),
        None,
    )?;

    Ok(stream)
}

#[cfg(target_os = "macos")]
mod macos_tap {
    use std::ffi::{c_void, CStr};
    use std::mem::{self, MaybeUninit};
    use std::ptr::{null, NonNull};
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
    use std::sync::Arc;

    use anyhow::{anyhow, bail, Result};
    use coreaudio::audio_unit::render_callback::{self, data};
    use coreaudio::audio_unit::{AudioUnit, Element, IOType, Scope};
    use objc2::rc::Retained;
    use objc2::{sel, AnyThread, ClassType};
    use objc2_audio_toolbox::{
        kAudioOutputUnitProperty_CurrentDevice, kAudioOutputUnitProperty_EnableIO,
        kAudioUnitProperty_StreamFormat,
    };
    use objc2_core_audio::{
        kAudioAggregateDeviceNameKey, kAudioAggregateDeviceTapAutoStartKey,
        kAudioAggregateDeviceTapListKey, kAudioAggregateDeviceUIDKey,
        kAudioDevicePropertyNominalSampleRate, kAudioDevicePropertyStreamFormat,
        kAudioEndPointDeviceIsPrivateKey, kAudioHardwarePropertyTranslatePIDToProcessObject,
        kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal,
        kAudioObjectPropertyScopeInput, kAudioObjectSystemObject, kAudioSubTapDriftCompensationKey,
        kAudioSubTapUIDKey, AudioHardwareCreateAggregateDevice, AudioHardwareCreateProcessTap,
        AudioHardwareDestroyAggregateDevice, AudioHardwareDestroyProcessTap,
        AudioObjectGetPropertyData, AudioObjectID, AudioObjectPropertyAddress, CATapDescription,
        CATapMuteBehavior,
    };
    use objc2_core_audio_types::{
        kAudioFormatFlagIsFloat, kAudioFormatFlagIsPacked, kAudioFormatLinearPCM, AudioBuffer,
        AudioStreamBasicDescription,
    };
    use objc2_core_foundation::{
        kCFAllocatorDefault, kCFTypeArrayCallBacks, kCFTypeDictionaryKeyCallBacks,
        kCFTypeDictionaryValueCallBacks, CFArray, CFDictionary, CFMutableDictionary, CFRetained,
        CFString,
    };
    use objc2_foundation::{
        ns_string, NSArray, NSNumber, NSOperatingSystemVersion, NSProcessInfo, NSString,
    };
    use ringbuf::traits::{Producer, Split};
    use ringbuf::HeapRb;

    use crate::audio::engine::StreamErrorNotifier;

    static TAP_INSTANCE_SEQ: AtomicU64 = AtomicU64::new(1);

    /// RAII guard for CoreAudio Process Tap + private Aggregate Device.
    struct TapObjectsGuard {
        tap_id: AudioObjectID,
        aggregate_id: Option<AudioObjectID>,
    }

    impl Drop for TapObjectsGuard {
        fn drop(&mut self) {
            unsafe {
                if let Some(agg_id) = self.aggregate_id.take() {
                    let _ = AudioHardwareDestroyAggregateDevice(agg_id);
                }
                if self.tap_id != 0 {
                    let _ = AudioHardwareDestroyProcessTap(self.tap_id);
                }
            }
        }
    }

    pub(crate) struct MacosProcessTapStream {
        // Declared BEFORE `_tap_guard` so `audio_unit` is stopped and dropped BEFORE
        // CoreAudio destroys the backing Aggregate Device and Process Tap.
        audio_unit: Option<AudioUnit>,
        _tap_guard: TapObjectsGuard,
    }

    impl Drop for MacosProcessTapStream {
        fn drop(&mut self) {
            if let Some(ref mut au) = self.audio_unit {
                let _ = au.stop();
            }
            let _ = self.audio_unit.take();
        }
    }

    fn to_cfstring(cstr: &'static CStr) -> CFRetained<CFString> {
        CFString::from_str(cstr.to_str().unwrap_or_default())
    }

    /// Translates Shizuka's own PID into a CoreAudio `AudioObjectID` so our own Live Monitor
    /// and shortcut chime playback are excluded from the speaker reference tap at the OS mixer.
    fn current_process_audio_object_id() -> Option<AudioObjectID> {
        let pid: i32 = std::process::id() as i32;
        let prop = AudioObjectPropertyAddress {
            mSelector: kAudioHardwarePropertyTranslatePIDToProcessObject,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain,
        };
        let mut obj_id: AudioObjectID = 0;
        let mut data_size = mem::size_of::<AudioObjectID>() as u32;
        let status = unsafe {
            AudioObjectGetPropertyData(
                kAudioObjectSystemObject as AudioObjectID,
                NonNull::from(&prop),
                mem::size_of::<i32>() as u32,
                (&pid as *const i32).cast(),
                NonNull::from(&mut data_size),
                NonNull::from(&mut obj_id).cast(),
            )
        };
        if status == 0 && obj_id != 0 {
            Some(obj_id)
        } else {
            None
        }
    }

    fn query_device_nominal_rate_and_channels(
        device_id: AudioObjectID,
        fallback_sr: u32,
    ) -> (u32, u32) {
        let mut sr: f64 = 0.0;
        let mut sr_size = mem::size_of::<f64>() as u32;
        let sr_prop = AudioObjectPropertyAddress {
            mSelector: kAudioDevicePropertyNominalSampleRate,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain,
        };
        let sr_status = unsafe {
            AudioObjectGetPropertyData(
                device_id,
                NonNull::from(&sr_prop),
                0,
                null(),
                NonNull::from(&mut sr_size),
                NonNull::from(&mut sr).cast(),
            )
        };
        let resolved_sr = if sr_status == 0 && sr >= 8_000.0 {
            sr.round() as u32
        } else {
            fallback_sr.max(8_000)
        };

        let mut asbd: AudioStreamBasicDescription = unsafe { mem::zeroed() };
        let mut asbd_size = mem::size_of::<AudioStreamBasicDescription>() as u32;
        let fmt_prop = AudioObjectPropertyAddress {
            mSelector: kAudioDevicePropertyStreamFormat,
            mScope: kAudioObjectPropertyScopeInput,
            mElement: kAudioObjectPropertyElementMain,
        };
        let fmt_status = unsafe {
            AudioObjectGetPropertyData(
                device_id,
                NonNull::from(&fmt_prop),
                0,
                null(),
                NonNull::from(&mut asbd_size),
                NonNull::from(&mut asbd).cast(),
            )
        };
        let resolved_ch = if fmt_status == 0 && asbd.mChannelsPerFrame >= 1 {
            asbd.mChannelsPerFrame.min(2)
        } else {
            2
        };

        (resolved_sr, resolved_ch)
    }

    fn create_private_aggregate_dict(
        tap_uid: &NSString,
        seq: u64,
    ) -> CFRetained<CFDictionary> {
        let true_num = NSNumber::new_bool(true);
        let false_num = NSNumber::new_bool(false);

        let tap_inner = unsafe {
            let dict = CFMutableDictionary::new(
                kCFAllocatorDefault,
                2,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            )
            .expect("CFMutableDictionary alloc");

            CFMutableDictionary::set_value(
                Some(dict.as_ref()),
                NonNull::from(&*to_cfstring(kAudioSubTapUIDKey))
                    .as_ptr()
                    .cast::<c_void>(),
                NonNull::from(tap_uid).as_ptr().cast::<c_void>(),
            );
            CFMutableDictionary::set_value(
                Some(dict.as_ref()),
                NonNull::from(&*to_cfstring(kAudioSubTapDriftCompensationKey))
                    .as_ptr()
                    .cast::<c_void>(),
                NonNull::from(&*true_num).as_ptr().cast::<c_void>(),
            );
            dict
        };

        let taps_slice = [tap_inner];
        let taps = unsafe {
            CFArray::new(
                kCFAllocatorDefault,
                taps_slice.as_ptr().cast::<*const c_void>().cast_mut(),
                taps_slice.len() as _,
                &kCFTypeArrayCallBacks,
            )
            .expect("CFArray alloc")
        };

        let agg_name = CFString::from_str("Shizuka AEC Reference Tap");
        let agg_uid = CFString::from_str(&format!("app.shizuka.audio.AecTapAggregate.{}", seq));

        unsafe {
            let dict = CFMutableDictionary::new(
                kCFAllocatorDefault,
                5,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            )
            .expect("CFMutableDictionary alloc");

            CFMutableDictionary::set_value(
                Some(dict.as_ref()),
                NonNull::from(&*to_cfstring(kAudioAggregateDeviceNameKey))
                    .as_ptr()
                    .cast::<c_void>(),
                NonNull::from(&*agg_name).as_ptr().cast::<c_void>(),
            );
            CFMutableDictionary::set_value(
                Some(dict.as_ref()),
                NonNull::from(&*to_cfstring(kAudioAggregateDeviceUIDKey))
                    .as_ptr()
                    .cast::<c_void>(),
                NonNull::from(&*agg_uid).as_ptr().cast::<c_void>(),
            );
            CFMutableDictionary::set_value(
                Some(dict.as_ref()),
                NonNull::from(&*to_cfstring(kAudioAggregateDeviceTapListKey))
                    .as_ptr()
                    .cast::<c_void>(),
                NonNull::from(&*taps).as_ptr().cast::<c_void>(),
            );
            CFMutableDictionary::set_value(
                Some(dict.as_ref()),
                NonNull::from(&*to_cfstring(kAudioAggregateDeviceTapAutoStartKey))
                    .as_ptr()
                    .cast::<c_void>(),
                NonNull::from(&*false_num).as_ptr().cast::<c_void>(),
            );
            CFMutableDictionary::set_value(
                Some(dict.as_ref()),
                NonNull::from(&*to_cfstring(kAudioEndPointDeviceIsPrivateKey))
                    .as_ptr()
                    .cast::<c_void>(),
                NonNull::from(&*true_num).as_ptr().cast::<c_void>(),
            );

            CFRetained::cast_unchecked::<CFDictionary>(dict)
        }
    }

    impl MacosProcessTapStream {
        pub(crate) fn open(
            device_uid: Option<&str>,
            fallback_sr: u32,
            observed_block_frames: Arc<AtomicU32>,
            _err_notifier: StreamErrorNotifier,
        ) -> Result<(Self, ringbuf::HeapCons<f32>, u32)> {
            // Runtime OS check (CoreAudio Process Tap requires macOS 14.2+; our minimumSystemVersion is 14.0)
            let is_14_2_plus = NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(
                NSOperatingSystemVersion {
                    majorVersion: 14,
                    minorVersion: 2,
                    patchVersion: 0,
                },
            );
            if !is_14_2_plus {
                bail!("macOS でエコー除去を使用するには macOS 14.2 以降が必要です");
            }

            let excluded_procs: Retained<NSArray<NSNumber>> =
                if let Some(self_obj) = current_process_audio_object_id() {
                    let num = NSNumber::new_u32(self_obj);
                    NSArray::from_retained_slice(&[num])
                } else {
                    NSArray::new()
                };

            // Prefer per-device tap (`initExcludingProcesses:andDeviceUID:withStream:`, macOS 14.6+)
            // even on duplex USB audio interfaces (e.g. Shure MV7 / MOTU) where `cpal`'s built-in
            // `build_input_stream` would mistakenly open the microphone input instead of speaker output!
            // Fall back to global stereo tap on macOS 14.2-14.5 via runtime `respondsToSelector:`.
            let tap_desc: Retained<CATapDescription> = unsafe {
                let cls = CATapDescription::class();
                if let Some(uid_str) = device_uid.filter(|s| !s.is_empty()) {
                    if cls.responds_to(sel!(initExcludingProcesses:andDeviceUID:withStream:)) {
                        let ns_uid = NSString::from_str(uid_str);
                        CATapDescription::initExcludingProcesses_andDeviceUID_withStream(
                            CATapDescription::alloc(),
                            &excluded_procs,
                            &ns_uid,
                            0,
                        )
                    } else if cls.responds_to(sel!(initStereoGlobalTapButExcludeProcesses:)) {
                        CATapDescription::initStereoGlobalTapButExcludeProcesses(
                            CATapDescription::alloc(),
                            &excluded_procs,
                        )
                    } else {
                        bail!("この macOS 環境は CoreAudio Process Tap API に対応していません");
                    }
                } else if cls.responds_to(sel!(initStereoGlobalTapButExcludeProcesses:)) {
                    CATapDescription::initStereoGlobalTapButExcludeProcesses(
                        CATapDescription::alloc(),
                        &excluded_procs,
                    )
                } else {
                    bail!("この macOS 環境は CoreAudio Process Tap API に対応していません");
                }
            };

            unsafe {
                tap_desc.setMuteBehavior(CATapMuteBehavior::Unmuted);
                tap_desc.setName(ns_string!("Shizuka AEC Reference"));
                tap_desc.setPrivate(true);
                tap_desc.setExclusive(true);
                tap_desc.setMixdown(true);
            }

            let mut tap_id_uninit: MaybeUninit<AudioObjectID> = MaybeUninit::uninit();
            let tap_status = unsafe {
                AudioHardwareCreateProcessTap(Some(tap_desc.as_ref()), tap_id_uninit.as_mut_ptr())
            };
            if tap_status != 0 {
                bail!(
                    "CoreAudio Process Tap の作成に失敗しました (OSStatus={})",
                    tap_status
                );
            }
            let tap_id = unsafe { tap_id_uninit.assume_init() };
            let mut tap_guard = TapObjectsGuard {
                tap_id,
                aggregate_id: None,
            };

            let tap_uid = unsafe { tap_desc.UUID().UUIDString() };
            let seq = TAP_INSTANCE_SEQ.fetch_add(1, Ordering::Relaxed);
            let agg_props = create_private_aggregate_dict(&tap_uid, seq);

            let mut agg_id: AudioObjectID = 0;
            let agg_status = unsafe {
                AudioHardwareCreateAggregateDevice(agg_props.as_ref(), NonNull::from(&mut agg_id))
            };
            if agg_status != 0 || agg_id == 0 {
                bail!(
                    "CoreAudio Aggregate Device の作成に失敗しました (OSStatus={})",
                    agg_status
                );
            }
            tap_guard.aggregate_id = Some(agg_id);

            let (sr_ref, ch_ref) = query_device_nominal_rate_and_channels(agg_id, fallback_sr);

            let mut audio_unit = AudioUnit::new(IOType::HalOutput)
                .map_err(|e| anyhow!("AUHAL 初期化失敗: {}", e))?;
            let enable_in = 1u32;
            audio_unit
                .set_property(
                    kAudioOutputUnitProperty_EnableIO,
                    Scope::Input,
                    Element::Input,
                    Some(&enable_in),
                )
                .map_err(|e| anyhow!("AUHAL 入力有効化失敗: {}", e))?;
            let disable_out = 0u32;
            audio_unit
                .set_property(
                    kAudioOutputUnitProperty_EnableIO,
                    Scope::Output,
                    Element::Output,
                    Some(&disable_out),
                )
                .map_err(|e| anyhow!("AUHAL 出力無効化失敗: {}", e))?;
            audio_unit
                .set_property(
                    kAudioOutputUnitProperty_CurrentDevice,
                    Scope::Global,
                    Element::Output,
                    Some(&agg_id),
                )
                .map_err(|e| anyhow!("AUHAL デバイス割当失敗: {}", e))?;

            let bytes_per_sample = mem::size_of::<f32>() as u32;
            let asbd = AudioStreamBasicDescription {
                mSampleRate: sr_ref as f64,
                mFormatID: kAudioFormatLinearPCM,
                mFormatFlags: kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked,
                mBytesPerPacket: bytes_per_sample * ch_ref,
                mFramesPerPacket: 1,
                mBytesPerFrame: bytes_per_sample * ch_ref,
                mChannelsPerFrame: ch_ref,
                mBitsPerChannel: 32,
                mReserved: 0,
            };
            audio_unit
                .set_property(
                    kAudioUnitProperty_StreamFormat,
                    Scope::Output,
                    Element::Input,
                    Some(&asbd),
                )
                .map_err(|e| anyhow!("AUHAL フォーマット設定失敗: {}", e))?;

            let rb_cap = (sr_ref as usize / 2).max(8_192);
            let rb = HeapRb::<f32>::new(rb_cap);
            let (mut prod, cons) = rb.split();
            let ch = (ch_ref as usize).max(1);

            type Args = render_callback::Args<data::Raw>;
            audio_unit
                .set_input_callback(move |args: Args| unsafe {
                    let AudioBuffer {
                        mNumberChannels: buf_ch,
                        mDataByteSize: byte_size,
                        mData: raw_ptr,
                    } = (*args.data.data).mBuffers[0];
                    if raw_ptr.is_null() || byte_size == 0 {
                        return Ok(());
                    }
                    let actual_ch = (buf_ch as usize).max(ch).max(1);
                    let sample_len = (byte_size as usize) / mem::size_of::<f32>();
                    let samples = std::slice::from_raw_parts(raw_ptr as *const f32, sample_len);

                    let mut mono_scratch = [0.0f32; 512];
                    let mut count = 0usize;
                    let mut total_frames = 0u32;

                    if actual_ch == 1 {
                        for &s in samples {
                            mono_scratch[count] = if s.is_finite() { s } else { 0.0 };
                            count += 1;
                            total_frames = total_frames.saturating_add(1);
                            if count == mono_scratch.len() {
                                let _ = prod.push_slice(&mono_scratch);
                                count = 0;
                            }
                        }
                    } else if actual_ch == 2 {
                        for frame in samples.as_chunks::<2>().0 {
                            let l = if frame[0].is_finite() { frame[0] } else { 0.0 };
                            let r = if frame[1].is_finite() { frame[1] } else { 0.0 };
                            mono_scratch[count] = (l + r) * 0.5;
                            count += 1;
                            total_frames = total_frames.saturating_add(1);
                            if count == mono_scratch.len() {
                                let _ = prod.push_slice(&mono_scratch);
                                count = 0;
                            }
                        }
                    } else {
                        let inv = 1.0 / (actual_ch.min(2) as f32);
                        for frame in samples.chunks_exact(actual_ch) {
                            let mut sum = 0.0f32;
                            for &v in &frame[..actual_ch.min(2)] {
                                if v.is_finite() {
                                    sum += v;
                                }
                            }
                            mono_scratch[count] = sum * inv;
                            count += 1;
                            total_frames = total_frames.saturating_add(1);
                            if count == mono_scratch.len() {
                                let _ = prod.push_slice(&mono_scratch);
                                count = 0;
                            }
                        }
                    }
                    if count > 0 {
                        let _ = prod.push_slice(&mono_scratch[..count]);
                    }
                    if total_frames > 0 {
                        observed_block_frames.fetch_max(total_frames, Ordering::Relaxed);
                    }
                    Ok(())
                })
                .map_err(|e| anyhow!("AUHAL コールバック登録失敗: {}", e))?;

            audio_unit
                .start()
                .map_err(|e| anyhow!("AUHAL ストリーム開始失敗: {}", e))?;

            Ok((
                Self {
                    audio_unit: Some(audio_unit),
                    _tap_guard: tap_guard,
                },
                cons,
                sr_ref,
            ))
        }
    }
}
