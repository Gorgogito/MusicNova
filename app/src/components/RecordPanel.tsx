import type { InputChannels, PracticeSettings, TransportEvent } from '../lib/api'
import { formatTime } from '../lib/format'
import type { Recorder } from '../lib/useRecorder'
import { Icon } from './Icon'
import { Meter } from './Meter'
import { plural, tr } from '../i18n'

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
    out.push({ value: `stereo:${c}`, label: tr('Estéreo · canales {a}-{b}', { a: c + 1, b: c + 2 }), channels: { mode: 'stereo', first: c } as InputChannels })
  return out
}

export function RecordPanel({ recorder, transport, practice, onPracticeChange }: Props) {
  const { input, prefs, devices, busy, calibration } = recorder
  const recording = transport.recording
  const device = devices.find((d) => d.name === prefs.device) ?? devices.find((d) => d.is_default)
  const options = channelOptions(Math.max(1, input?.device_channels ?? device?.channels ?? 2))
  const channelValue = `${prefs.channels.mode}:${prefs.channels.first}`

  const blockers = [
    practice.tempo !== 1 && tr('velocidad al {pct} %', { pct: Math.round(practice.tempo * 100) }),
    practice.pitch !== 0 && tr('tono {st} st', { st: `${practice.pitch > 0 ? '+' : ''}${practice.pitch}` }),
    practice.loop_enabled && tr('bucle A-B activo'),
  ].filter(Boolean) as string[]

  const changeInput = (patch: Parameters<Recorder['setPrefs']>[0]) => {
    recorder.setPrefs(patch)
    if (input && !recording) recorder.open()
  }

  const status = recording
    ? transport.counting_in
      ? tr('Cuenta previa…')
      : tr('Grabando · {time}', { time: formatTime(transport.recorded) })
    : input
      ? tr('Lista para grabar')
      : tr('Activa una entrada para grabar')

  return (
    <div className="record">
      <section className="practice-group">
        <header>
          <strong>{tr('Entrada')}</strong>
          <button className="icon-btn" title={tr('Buscar dispositivos')} onClick={recorder.refreshDevices} disabled={recording}>
            <Icon name="refresh" size={16} />
          </button>
        </header>
        <label className="field">
          <span className="small muted">{tr('Dispositivo')}</span>
          <select
            value={prefs.device ?? ''}
            disabled={recording}
            onChange={(e) => changeInput({ device: e.target.value || null, channels: { mode: 'mono', first: 0 } })}
          >
            <option value="">{tr('Predeterminado del sistema')}{devices.find((d) => d.is_default) ? ` (${devices.find((d) => d.is_default)!.name})` : ''}</option>
            {devices.map((d) => (
              <option key={d.name} value={d.name}>
                {d.name} · {plural(d.channels, '{n} canal', '{n} canales')}
              </option>
            ))}
          </select>
        </label>
        <label className="field">
          <span className="small muted">{tr('Canales')}</span>
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
              <Meter left={transport.input_peak?.[0] ?? 0} right={transport.input_peak?.[1] ?? 0} title={tr('Nivel de entrada')} />
              <button className="btn ghost small" onClick={recorder.close} disabled={recording}>
                {tr('Desactivar')}
              </button>
            </>
          ) : (
            <button className="btn primary" onClick={recorder.open} disabled={!!busy}>
              <Icon name="mic" size={16} /> {tr('Activar entrada')}
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
                  <label className="lane-slider" title={tr('Volumen de la monitorización (doble clic: 0 dB)')}>
                    <span className="small muted">{tr('Vol')}</span>
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
                    {tr('Usa auriculares para evitar acoples. Si tu interfaz tiene monitorización directa, es mejor: no añade retraso.')}
                  </p>
                </>
              )
            ) : (
              <p className="small muted">
                {tr('La entrada funciona a {khz} kHz y la salida a otra frecuencia: no se puede escuchar en directo, pero la toma se convierte al terminar.', { khz: input.sample_rate / 1000 })}
              </p>
            )}
          </>
        )}
      </section>

      <section className="practice-group">
        <header>
          <strong>{tr('Compensación de latencia')}</strong>
        </header>
        <p className="small muted">
          {tr('Lo que grabas llega con retraso respecto a lo que oyes. Para medirlo, pon el micrófono cerca de los altavoces (o conecta la salida a la entrada con un cable), sube el volumen y pulsa Calibrar: sonarán 8 barridos cortos.')}
        </p>
        <div className="panel-row">
          <label className="field">
            <span className="small muted">{tr('Latencia (ms)')}</span>
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
            <Icon name="refresh" size={16} /> {busy === 'Midiendo la latencia…' ? tr('Midiendo…') : tr('Calibrar')}
          </button>
        </div>
        {calibration ? (
          <p className="small">
            {tr('Medido:')} <strong className="mono">{calibration.latency_ms.toFixed(1)} ms</strong> ·{' '}
            {tr('{detected}/{total} barridos, dispersión {spread} ms, nivel {level} dBFS', {
              detected: calibration.detected,
              total: calibration.total,
              spread: calibration.spread_ms.toFixed(2),
              level: calibration.level_db.toFixed(0),
            })}
          </p>
        ) : (
          prefs.latencyMs !== 0 && (
            <p className="small muted">
              {tr('Valor de una sesión anterior. El retraso cambia unos milisegundos cada vez que se abre la app: calibra de nuevo para la máxima precisión.')}
            </p>
          )
        )}
      </section>

      <section className="practice-group">
        <header>
          <strong>{tr('Grabar')}</strong>
          <span className={`small ${recording ? 'rec-status' : 'muted'}`}>{status}</span>
        </header>
        {blockers.length > 0 && !recording && (
          <div className="notice small">
            {tr('Para grabar en sincronía hace falta reproducir a velocidad y tono originales, sin bucle ({list}).', { list: blockers.join(', ') })}{' '}
            <button className="link small" onClick={() => onPracticeChange({ tempo: 1, pitch: 0, loop_enabled: false })}>
              {tr('Ajustar')}
            </button>
          </div>
        )}
        <div className="panel-row">
          {recording ? (
            <button className="btn record-btn on" onClick={recorder.stop}>
              <Icon name="stop" size={16} /> {tr('Detener')}
            </button>
          ) : (
            <button
              className="btn record-btn"
              disabled={!input || blockers.length > 0 || !!busy}
              onClick={() => recorder.start(transport.position)}
              title={tr('Grabar desde la posición actual (R)')}
            >
              <Icon name="record" size={16} /> {tr('Grabar')}
            </button>
          )}
          <label className="field">
            <span className="small muted">{tr('Cuenta previa')}</span>
            <select
              className="compact"
              value={practice.count_in_bars}
              disabled={recording}
              onChange={(e) => onPracticeChange({ count_in_bars: Number(e.target.value) })}
            >
              <option value={0}>{tr('Sin cuenta')}</option>
              <option value={1}>{tr('1 compás')}</option>
              <option value={2}>{tr('2 compases')}</option>
            </select>
          </label>
          <label className="check">
            <input type="checkbox" checked={practice.metronome} onChange={(e) => onPracticeChange({ metronome: e.target.checked })} />
            {tr('Metrónomo')}
          </label>
        </div>
        <p className="small muted">
          {tr('La toma empieza en la posición actual ({time}) y se añade como pista nueva al detener (barra espaciadora o R). Las tomas se guardan en la carpeta audio/recordings del proyecto.', {
            time: formatTime(recorder.startedAt ?? transport.position),
          })}
        </p>
      </section>
    </div>
  )
}
