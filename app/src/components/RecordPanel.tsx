import type { InputChannels, PracticeSettings, TransportEvent } from '../lib/api'
import { formatTime } from '../lib/format'
import type { Recorder } from '../lib/useRecorder'
import { Icon } from './Icon'
import { Meter } from './Meter'

interface Props {
  recorder: Recorder
  transport: TransportEvent
  practice: PracticeSettings
  onPracticeChange: (patch: Partial<PracticeSettings>) => void
}

/** Opciones de canales para un dispositivo con `n` entradas. */
function channelOptions(n: number): { value: string; label: string; channels: InputChannels }[] {
  const out = []
  for (let c = 0; c < n; c++) out.push({ value: `mono:${c}`, label: `Mono · canal ${c + 1}`, channels: { mode: 'mono', first: c } as InputChannels })
  for (let c = 0; c + 1 < n; c += 2)
    out.push({ value: `stereo:${c}`, label: `Estéreo · canales ${c + 1}-${c + 2}`, channels: { mode: 'stereo', first: c } as InputChannels })
  return out
}

export function RecordPanel({ recorder, transport, practice, onPracticeChange }: Props) {
  const { input, prefs, devices, busy, calibration } = recorder
  const recording = transport.recording
  const device = devices.find((d) => d.name === prefs.device) ?? devices.find((d) => d.is_default)
  const options = channelOptions(Math.max(1, input?.device_channels ?? device?.channels ?? 2))
  const channelValue = `${prefs.channels.mode}:${prefs.channels.first}`

  const blockers = [
    practice.tempo !== 1 && `velocidad al ${Math.round(practice.tempo * 100)} %`,
    practice.pitch !== 0 && `tono ${practice.pitch > 0 ? '+' : ''}${practice.pitch} st`,
    practice.loop_enabled && 'bucle A-B activo',
  ].filter(Boolean) as string[]

  const changeInput = (patch: Parameters<Recorder['setPrefs']>[0]) => {
    recorder.setPrefs(patch)
    if (input && !recording) recorder.open()
  }

  const status = recording
    ? transport.counting_in
      ? 'Cuenta previa…'
      : `Grabando · ${formatTime(transport.recorded)}`
    : input
      ? 'Lista para grabar'
      : 'Activa una entrada para grabar'

  return (
    <div className="record">
      <section className="practice-group">
        <header>
          <strong>Entrada</strong>
          <button className="icon-btn" title="Buscar dispositivos" onClick={recorder.refreshDevices} disabled={recording}>
            <Icon name="refresh" size={16} />
          </button>
        </header>
        <label className="field">
          <span className="small muted">Dispositivo</span>
          <select
            value={prefs.device ?? ''}
            disabled={recording}
            onChange={(e) => changeInput({ device: e.target.value || null, channels: { mode: 'mono', first: 0 } })}
          >
            <option value="">Predeterminado del sistema{devices.find((d) => d.is_default) ? ` (${devices.find((d) => d.is_default)!.name})` : ''}</option>
            {devices.map((d) => (
              <option key={d.name} value={d.name}>
                {d.name} · {d.channels} {d.channels === 1 ? 'canal' : 'canales'}
              </option>
            ))}
          </select>
        </label>
        <label className="field">
          <span className="small muted">Canales</span>
          <select
            value={channelValue}
            disabled={recording}
            onChange={(e) => {
              const o = options.find((x) => x.value === e.target.value)
              if (o) changeInput({ channels: o.channels })
            }}
          >
            {options.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
        </label>
        <div className="panel-row">
          {input ? (
            <>
              <span className="small">
                <span className="dot ready" /> {input.sample_rate / 1000} kHz
              </span>
              <Meter left={transport.input_peak?.[0] ?? 0} right={transport.input_peak?.[1] ?? 0} title="Nivel de entrada" />
              <button className="btn ghost small" onClick={recorder.close} disabled={recording}>
                Desactivar
              </button>
            </>
          ) : (
            <button className="btn primary" onClick={recorder.open} disabled={!!busy}>
              <Icon name="mic" size={16} /> Activar entrada
            </button>
          )}
        </div>
        {input && (
          <>
            <label className={`check ${input.monitor_available ? '' : 'disabled'}`}>
              <input
                type="checkbox"
                checked={recorder.monitor && input.monitor_available}
                disabled={!input.monitor_available}
                onChange={(e) => recorder.setMonitor(e.target.checked)}
              />
              Escuchar la entrada (monitorización)
            </label>
            {input.monitor_available ? (
              recorder.monitor && (
                <>
                  <label className="lane-slider" title="Volumen de la monitorización (doble clic: 0 dB)">
                    <span className="small muted">Vol</span>
                    <input
                      type="range"
                      min={-40}
                      max={6}
                      step={0.5}
                      value={prefs.monitorGainDb}
                      onChange={(e) => recorder.setMonitor(true, Number(e.target.value))}
                      onDoubleClick={() => recorder.setMonitor(true, 0)}
                    />
                    <span className="small mono">{prefs.monitorGainDb.toFixed(1)} dB</span>
                  </label>
                  <p className="small muted">
                    Usa auriculares para evitar acoples. Si tu interfaz tiene monitorización directa, es mejor: no añade
                    retraso.
                  </p>
                </>
              )
            ) : (
              <p className="small muted">
                La entrada funciona a {input.sample_rate / 1000} kHz y la salida a otra frecuencia: no se puede escuchar
                en directo, pero la toma se convierte al terminar.
              </p>
            )}
          </>
        )}
      </section>

      <section className="practice-group">
        <header>
          <strong>Compensación de latencia</strong>
        </header>
        <p className="small muted">
          Lo que grabas llega con retraso respecto a lo que oyes. Para medirlo, pon el micrófono cerca de los altavoces
          (o conecta la salida a la entrada con un cable), sube el volumen y pulsa Calibrar: sonarán 8 barridos cortos.
        </p>
        <div className="panel-row">
          <label className="field">
            <span className="small muted">Latencia (ms)</span>
            <input
              type="number"
              className="num wide"
              step={0.1}
              min={-500}
              max={1000}
              value={prefs.latencyMs}
              disabled={recording}
              onChange={(e) => recorder.setPrefs({ latencyMs: Number(e.target.value) || 0 })}
            />
          </label>
          <button className="btn" onClick={recorder.calibrate} disabled={!input || recording || transport.playing || !!busy}>
            <Icon name="refresh" size={16} /> {busy === 'Midiendo la latencia…' ? 'Midiendo…' : 'Calibrar'}
          </button>
        </div>
        {calibration ? (
          <p className="small">
            Medido: <strong className="mono">{calibration.latency_ms.toFixed(1)} ms</strong> · {calibration.detected}/
            {calibration.total} barridos, dispersión {calibration.spread_ms.toFixed(2)} ms, nivel{' '}
            {calibration.level_db.toFixed(0)} dBFS
          </p>
        ) : (
          prefs.latencyMs !== 0 && (
            <p className="small muted">
              Valor de una sesión anterior. El retraso cambia unos milisegundos cada vez que se abre la app: calibra de
              nuevo para la máxima precisión.
            </p>
          )
        )}
      </section>

      <section className="practice-group">
        <header>
          <strong>Grabar</strong>
          <span className={`small ${recording ? 'rec-status' : 'muted'}`}>{status}</span>
        </header>
        {blockers.length > 0 && !recording && (
          <div className="notice small">
            Para grabar en sincronía hace falta reproducir a velocidad y tono originales, sin bucle ({blockers.join(', ')}).{' '}
            <button className="link small" onClick={() => onPracticeChange({ tempo: 1, pitch: 0, loop_enabled: false })}>
              Ajustar
            </button>
          </div>
        )}
        <div className="panel-row">
          {recording ? (
            <button className="btn record-btn on" onClick={recorder.stop}>
              <Icon name="stop" size={16} /> Detener
            </button>
          ) : (
            <button
              className="btn record-btn"
              disabled={!input || blockers.length > 0 || !!busy}
              onClick={() => recorder.start(transport.position)}
              title="Grabar desde la posición actual (R)"
            >
              <Icon name="record" size={16} /> Grabar
            </button>
          )}
          <label className="field">
            <span className="small muted">Cuenta previa</span>
            <select
              className="compact"
              value={practice.count_in_bars}
              disabled={recording}
              onChange={(e) => onPracticeChange({ count_in_bars: Number(e.target.value) })}
            >
              <option value={0}>Sin cuenta</option>
              <option value={1}>1 compás</option>
              <option value={2}>2 compases</option>
            </select>
          </label>
          <label className="check">
            <input type="checkbox" checked={practice.metronome} onChange={(e) => onPracticeChange({ metronome: e.target.checked })} />
            Metrónomo
          </label>
        </div>
        <p className="small muted">
          La toma empieza en la posición actual ({formatTime(recorder.startedAt ?? transport.position)}) y se añade como
          pista nueva al detener (barra espaciadora o R). Las tomas se guardan en la carpeta audio/recordings del
          proyecto.
        </p>
      </section>
    </div>
  )
}
