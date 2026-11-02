import { api, errorMessage, type PracticeSettings, type TransportEvent } from '../lib/api'
import { formatTime } from '../lib/format'
import { Icon } from './Icon'
import { Meter } from './Meter'

interface Props {
  transport: TransportEvent
  enabled: boolean
  practice?: PracticeSettings
  onError: (m: string) => void
}

export function TransportBar({ transport, enabled, practice, onError }: Props) {
  const run = (p: Promise<unknown>) => p.catch((e) => onError(errorMessage(e)))
  const { position, duration, playing } = transport
  const pct = duration > 0 ? (position / duration) * 100 : 0

  return (
    <footer className="transport">
      <div className="transport-buttons">
        <button className="icon-btn" title="Ir al inicio (Inicio)" disabled={!enabled || transport.recording} onClick={() => run(api.seek(0))}>
          <Icon name="back" />
        </button>
        <button
          className="icon-btn play"
          title={transport.recording ? 'Detener la grabación (Espacio)' : playing ? 'Pausa (Espacio)' : 'Reproducir (Espacio)'}
          disabled={!enabled}
          onClick={() => run(playing || transport.counting_in ? api.pause() : api.play())}
        >
          <Icon name={playing ? 'pause' : 'play'} size={22} />
        </button>
        <button
          className="icon-btn"
          title="Detener"
          disabled={!enabled}
          onClick={() => run(transport.recording ? api.pause() : api.pause().then(() => api.seek(0)))}
        >
          <Icon name="stop" />
        </button>
      </div>

      <div className="transport-time mono">
        <span className="time-now">{formatTime(position)}</span>
        <span className="muted"> / {formatTime(duration)}</span>
        <div className="transport-badges">
          {transport.recording && !transport.counting_in && <span className="badge rec">● REC {formatTime(transport.recorded)}</span>}
          {transport.counting_in && <span className="badge warn">Cuenta previa…</span>}
          {practice && practice.tempo !== 1 && <span className="badge">{Math.round(practice.tempo * 100)}%</span>}
          {practice && practice.pitch !== 0 && (
            <span className="badge">
              {practice.pitch > 0 ? '+' : ''}
              {practice.pitch} st
            </span>
          )}
          {practice?.loop_enabled && <span className="badge ok">Bucle</span>}
          {practice?.metronome && <span className="badge">Metrónomo</span>}
        </div>
      </div>

      <input
        className="scrubber"
        type="range"
        min={0}
        max={Math.max(duration, 0.001)}
        step={0.01}
        value={Math.min(position, duration)}
        disabled={!enabled || transport.recording}
        style={{ ['--pct' as string]: `${pct}%` }}
        onChange={(e) => run(api.seek(Number(e.target.value)))}
        aria-label="Posición"
      />

      <Meter left={transport.peak_l} right={transport.peak_r} />
    </footer>
  )
}
