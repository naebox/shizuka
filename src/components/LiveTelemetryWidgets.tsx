import { useLiveTelemetry } from "../telemetryStore";
import type { ConnectionState } from "../types";

export function dbToRatio(db: number): number {
  const clamped = Math.max(-60, Math.min(0, db));
  return (clamped + 60) / 60;
}

export function TopBarStatPills() {
  const telemetry = useLiveTelemetry();
  const cpuPct = telemetry?.cpu_load_pct ?? 0;
  const cpuText = cpuPct >= 99.95 ? "100%" : `${cpuPct.toFixed(1)}%`;
  const latencyMs = Math.min(999, Math.max(0, Math.round(telemetry?.latency_ms ?? 10)));

  return (
    <>
      <span className="stat-pill stat-pill-cpu">
        <span>CPU</span>
        <span className="stat-pill-val">
          <span className="stat-pill-sizer" aria-hidden="true">
            99.9%
          </span>
          <span className="stat-pill-live">{cpuText}</span>
        </span>
      </span>
      <span className="stat-pill stat-pill-ms">
        <span className="stat-pill-val">
          <span className="stat-pill-sizer" aria-hidden="true">
            999
          </span>
          <span className="stat-pill-live">{latencyMs}</span>
        </span>
        <span>ms</span>
      </span>
    </>
  );
}

export function HeroLiveMeters({
  inputMeterId,
  outputMeterId,
  ncEnabled,
  micMuted,
}: {
  inputMeterId: string;
  outputMeterId: string;
  ncEnabled: boolean;
  micMuted: boolean;
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
          <label htmlFor={inputMeterId}>入力マイク (処理前)</label>
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
                ? "ミュート中 (無音送信)"
                : ncEnabled
                ? "ノイズ除去後"
                : "スルー出力 (未処理)"}
            </label>
            {!micMuted && ncEnabled && reducedDb > 1.5 && (
              <span className="reduction-badge">-{reducedDb.toFixed(0)} dB</span>
            )}
            <span role="status" aria-live="polite">
              {clippingActive && (
                <span
                  className="clip-badge"
                  title="入力または出力がリミッターに達しました。入力ゲインを下げてください。"
                >
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
          aria-valuetext={micMuted ? "ミュート" : `${outDb.toFixed(0)} dB`}
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
}: {
  liveMonitorEnabled: boolean;
}) {
  const telemetry = useLiveTelemetry();
  const aecActive = telemetry?.aec_active ?? false;
  const erleDb = telemetry?.aec_erle_db ?? 0;
  const label = liveMonitorEnabled
    ? "テスト中停止"
    : aecActive
    ? erleDb >= 2.0
      ? `稼働中 · -${erleDb.toFixed(0)} dB`
      : "稼働中"
    : "待機中";

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
}: {
  vadGateEnabled: boolean;
  ncEnabled: boolean;
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
          ? "待機"
          : vadSpeaking
          ? "話し中"
          : "消音中"}
      </span>
    </span>
  );
}

export function VoiceGateLiveBadge({
  voiceGateEnabled,
  hasVoiceProfile,
  ncEnabled,
}: {
  voiceGateEnabled: boolean;
  hasVoiceProfile: boolean;
  ncEnabled: boolean;
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
    ? "待機"
    : !hasVoiceProfile
    ? learnPct > 0
      ? `学習中 ${learnPct}%`
      : "声を学習中"
    : blocking
    ? "周囲の声を消音中"
    : speaking
    ? "話し中"
    : "待機中";

  return (
    <span className={`vad-live-pill ${stateClass}`} aria-hidden="true">
      <span className="vad-live-dot" />
      <span>{label}</span>
    </span>
  );
}

export function VoiceAutoLearnProgress({
  progressId,
}: {
  progressId: string;
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
          普段通りに話すと自動で声を覚えます
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
      <span className="field-hint">
        マイクの近くで数秒間会話すると記憶が完了します（話している間だけ進みます）
      </span>
    </div>
  );
}

export function MicTestLiveMeter({
  monMeterId,
  liveMonitorEnabled,
}: {
  monMeterId: string;
  liveMonitorEnabled: boolean;
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
      aria-valuetext={liveMonitorEnabled ? `${monDb.toFixed(0)} dB` : "停止中"}
    />
  );
}

export function SettingsDiagGrid({
  connState,
}: {
  connState: ConnectionState;
}) {
  const telemetry = useLiveTelemetry();
  const connStateLabel =
    connState === "running"
      ? "稼働中"
      : connState === "waiting_device"
      ? "再接続待ち"
      : connState === "reconnecting"
      ? "再接続中"
      : "停止中";
  const driftPpm = telemetry?.drift_ppm ?? 0;

  return (
    <div className="diag-grid">
      <div className="diag-item">
        <span className="diag-key">パイプライン状態</span>
        <span className="diag-val">{connStateLabel}</span>
      </div>
      <div className="diag-item">
        <span className="diag-key">クロック同期補正</span>
        <span className="diag-val">
          {driftPpm >= 0 ? "+" : ""}
          {driftPpm.toFixed(0)} ppm
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
            ? `${Math.round(telemetry.status.input_sample_rate / 1000)} kHz`
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
  );
}
