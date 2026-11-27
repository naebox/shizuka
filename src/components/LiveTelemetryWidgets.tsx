import type { UiMessages } from "../i18n";
import { useLiveTelemetry } from "../telemetryStore";
import type { ConnectionState } from "../types";

export function dbToRatio(db: number): number {
  const clamped = Math.max(-60, Math.min(0, db));
  return (clamped + 60) / 60;
}

export function HeroInlineStats() {
  const telemetry = useLiveTelemetry();
  const cpuPct = telemetry?.cpu_load_pct ?? 0;
  const cpuText = cpuPct >= 99.95 ? "100%" : `${cpuPct.toFixed(1)}%`;
  const latencyMs = Math.min(
    999,
    Math.max(0, Math.round(telemetry?.latency_ms ?? 10))
  );

  return (
    <span
      className="hero-inline-stats"
      aria-label={`CPU ${cpuText}, ${latencyMs} ms`}
    >
      <span className="hero-stat-item">
        <span>CPU</span>
        <span className="stat-pill-val">
          <span className="stat-pill-sizer" aria-hidden="true">
            99.9%
          </span>
          <span className="stat-pill-live">{cpuText}</span>
        </span>
      </span>
      <span className="hero-stat-dot" aria-hidden="true">
        ·
      </span>
      <span className="hero-stat-item">
        <span className="stat-pill-val">
          <span className="stat-pill-sizer" aria-hidden="true">
            99
          </span>
          <span className="stat-pill-live">{latencyMs}</span>
        </span>
        <span>ms</span>
      </span>
    </span>
  );
}

export function HeroLiveMeters({
  inputMeterId,
  outputMeterId,
  ncEnabled,
  micMuted,
  t,
}: {
  inputMeterId: string;
  outputMeterId: string;
  ncEnabled: boolean;
  micMuted: boolean;
  t: UiMessages;
}) {
  const telemetry = useLiveTelemetry();
  const inDb = telemetry?.input_rms_db ?? -90;
  const outDb = telemetry?.output_rms_db ?? -90;
  const reducedDb = telemetry?.noise_reduced_db ?? 0;
  const clippingActive = telemetry?.clipping_active ?? false;

  return (
    <div className="meters-box">
      <div className="meter-row">
        <div className="meter-label">
          <label htmlFor={inputMeterId}>{t.hero.meterInputRaw}</label>
          <span>{inDb.toFixed(0)} dB</span>
        </div>
        <progress
          id={inputMeterId}
          className="audio-meter raw"
          max={1}
          value={dbToRatio(inDb)}
          aria-valuetext={`${inDb.toFixed(0)} dB`}
        />
      </div>

      <div className="meter-row">
        <div className="meter-label">
          <span className="meter-label-left">
            <label htmlFor={outputMeterId}>
              {micMuted
                ? t.hero.meterMutedSilence
                : ncEnabled
                ? t.hero.meterDenoised
                : t.hero.meterBypassRaw}
            </label>
            {!micMuted && ncEnabled && reducedDb > 1.5 && (
              <span className="reduction-badge">-{reducedDb.toFixed(0)} dB</span>
            )}
            <span role="status" aria-live="polite">
              {clippingActive && (
                <span className="clip-badge" title={t.hero.clipTooltip}>
                  CLIP
                </span>
              )}
            </span>
          </span>
          <span>{micMuted ? "-∞ dB" : `${outDb.toFixed(0)} dB`}</span>
        </div>
        <progress
          id={outputMeterId}
          className={`audio-meter ${
            micMuted ? "muted" : ncEnabled ? "" : "raw"
          }`}
          max={1}
          value={micMuted ? 0 : dbToRatio(outDb)}
          aria-valuetext={micMuted ? t.hero.meterMutedAria : `${outDb.toFixed(0)} dB`}
        />
      </div>
    </div>
  );
}

export function AgcLiveBadge() {
  const telemetry = useLiveTelemetry();
  const agcGainDb = telemetry?.agc_gain_db ?? 0;
  const text =
    agcGainDb >= 0
      ? `+${agcGainDb.toFixed(1)} dB`
      : `${agcGainDb.toFixed(1)} dB`;
  return (
    <span className="agc-live-pill" aria-hidden="true">
      {text}
    </span>
  );
}

export function AecLiveBadge({
  liveMonitorEnabled,
  t,
}: {
  liveMonitorEnabled: boolean;
  t: UiMessages;
}) {
  const telemetry = useLiveTelemetry();
  const aecActive = telemetry?.aec_active ?? false;
  const erleDb = telemetry?.aec_erle_db ?? 0;
  const label = liveMonitorEnabled
    ? t.tuning.aecPausedInTest
    : aecActive
    ? erleDb >= 2.0
      ? t.tuning.aecActiveWithErle(erleDb.toFixed(0))
      : t.tuning.aecActive
    : t.tuning.aecStandby;

  return (
    <span
      className={`aec-live-pill ${
        aecActive && !liveMonitorEnabled ? "is-active" : ""
      }`}
      aria-hidden="true"
    >
      <span className="vad-live-dot" />
      <span>{label}</span>
    </span>
  );
}

export function VadLiveBadge({
  vadGateEnabled,
  ncEnabled,
  t,
}: {
  vadGateEnabled: boolean;
  ncEnabled: boolean;
  t: UiMessages;
}) {
  const telemetry = useLiveTelemetry();
  const vadSpeaking = telemetry?.vad_speaking ?? false;
  return (
    <span
      className={`vad-live-pill ${
        vadGateEnabled && ncEnabled && vadSpeaking ? "speaking" : ""
      }`}
      aria-hidden="true"
    >
      <span className="vad-live-dot" />
      <span>
        {!vadGateEnabled
          ? "OFF"
          : !ncEnabled
          ? t.tuning.vadStandby
          : vadSpeaking
          ? t.tuning.vadSpeaking
          : t.tuning.vadMuted}
      </span>
    </span>
  );
}

export function VoiceGateLiveBadge({
  voiceGateEnabled,
  hasVoiceProfile,
  ncEnabled,
  t,
}: {
  voiceGateEnabled: boolean;
  hasVoiceProfile: boolean;
  ncEnabled: boolean;
  t: UiMessages;
}) {
  const telemetry = useLiveTelemetry();
  const blocking = telemetry?.voice_gate_blocking ?? false;
  const speaking = telemetry?.vad_speaking ?? false;
  const learnPct = Math.round(
    Math.max(0, Math.min(1, telemetry?.voice_learn_progress ?? 0)) * 100
  );

  const stateClass =
    !voiceGateEnabled || !ncEnabled
      ? ""
      : !hasVoiceProfile
      ? "learning"
      : blocking
      ? "blocking"
      : speaking
      ? "speaking"
      : "";

  const label = !voiceGateEnabled
    ? "OFF"
    : !ncEnabled
    ? t.tuning.vadStandby
    : !hasVoiceProfile
    ? learnPct > 0
      ? t.tuning.voiceGateLearningPct(learnPct)
      : t.tuning.voiceGateLearning
    : blocking
    ? t.tuning.voiceGateBlocking
    : speaking
    ? t.tuning.vadSpeaking
    : t.tuning.aecStandby;

  return (
    <span className={`vad-live-pill ${stateClass}`} aria-hidden="true">
      <span className="vad-live-dot" />
      <span>{label}</span>
    </span>
  );
}

export function VoiceAutoLearnProgress({
  progressId,
  t,
}: {
  progressId: string;
  t: UiMessages;
}) {
  const telemetry = useLiveTelemetry();
  const progress = Math.max(
    0,
    Math.min(1, telemetry?.voice_learn_progress ?? 0)
  );
  const pct = Math.round(progress * 100);

  return (
    <div className="voice-enroll-active">
      <div className="slider-top">
        <label htmlFor={progressId} className="voice-enroll-prompt">
          {t.tuning.voiceAutoLearnPrompt}
        </label>
        <span className="slider-val">{pct}%</span>
      </div>
      <progress
        id={progressId}
        className="update-progress"
        max={1}
        value={progress}
        aria-valuetext={`${pct}%`}
      />
      <span className="field-hint">{t.tuning.voiceAutoLearnHint}</span>
    </div>
  );
}

export function MicTestLiveMeter({
  monMeterId,
  liveMonitorEnabled,
  t,
}: {
  monMeterId: string;
  liveMonitorEnabled: boolean;
  t: UiMessages;
}) {
  const telemetry = useLiveTelemetry();
  const monDb = telemetry?.monitor_rms_db ?? -90;
  const ratio = liveMonitorEnabled ? dbToRatio(monDb) : 0;
  return (
    <progress
      id={monMeterId}
      className="discord-meter"
      max={1}
      value={ratio}
      aria-valuetext={
        liveMonitorEnabled ? `${monDb.toFixed(0)} dB` : t.micTest.stoppedAria
      }
    />
  );
}

export function SettingsDiagGrid({
  connState,
  t,
}: {
  connState: ConnectionState;
  t: UiMessages;
}) {
  const telemetry = useLiveTelemetry();
  const connStateLabel =
    connState === "running"
      ? t.settings.diagStateRunning
      : connState === "waiting_device"
      ? t.settings.diagStateWaiting
      : connState === "reconnecting"
      ? t.settings.diagStateReconnecting
      : t.settings.diagStateStopped;
  const driftPpm = telemetry?.drift_ppm ?? 0;

  return (
    <div className="diag-grid">
      <div className="diag-item">
        <span className="diag-key">{t.settings.diagPipelineStatus}</span>
        <span className="diag-val">{connStateLabel}</span>
      </div>
      <div className="diag-item">
        <span className="diag-key">{t.settings.diagClockSync}</span>
        <span className="diag-val">
          {driftPpm >= 0 ? "+" : ""}
          {driftPpm.toFixed(0)} ppm
        </span>
      </div>
      <div className="diag-item">
        <span className="diag-key">{t.settings.diagReconnects}</span>
        <span className="diag-val">
          {t.settings.diagReconnectValue(
            telemetry?.status.reconnect_count ?? 0
          )}
        </span>
      </div>
      <div className="diag-item">
        <span className="diag-key">{t.settings.diagSampleRate}</span>
        <span className="diag-val">
          {telemetry?.status.input_sample_rate
            ? `${Math.round(telemetry.status.input_sample_rate / 1000)} kHz`
            : "—"}
        </span>
      </div>
      <div className="diag-item">
        <span className="diag-key">{t.settings.diagInputDrops}</span>
        <span className="diag-val">
          {telemetry?.status.input_drop_count ?? 0}
        </span>
      </div>
      <div className="diag-item">
        <span className="diag-key">{t.settings.diagOutputUnderruns}</span>
        <span className="diag-val">
          {telemetry?.status.output_underrun_count ?? 0}
        </span>
      </div>
    </div>
  );
}
