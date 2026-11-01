import { useRef, useState } from 'react'
import type { PracticeSettings } from '../lib/api'
import { formatTime } from '../lib/format'
import { Icon } from './Icon'

interface Props {
  practice: PracticeSettings
  position: number
  duration: number
  onChange: (patch: Partial<PracticeSettings>) => void
  onSeek: (t: number) => void
}

const pct = (v: number) => `${Math.round(v * 100)}%`
const signed = (v: number) => (v > 0 ? `+${v}` : `${v}`)

export function PracticePanel({ practice: p, position, duration, onChange, onSeek }: Props) {
  const taps = useRef<number[]>([])
  const [editing, setEditing] = useState<string | null>(null)

  const tap = () => {
    const now = performance.now()
    const recent = [...taps.current.filter((t) => now - t < 3000), now]
    taps.current = recent
    if (recent.length >= 3) {
      const intervals = recent.slice(1).map((t, i) => t - recent[i])
      const avg = intervals.reduce((a, b) => a + b, 0) / intervals.length
      onChange({ bpm: Math.round((60000 / avg) * 10) / 10 })
    }
  }

  const setLoopPoint = (which: 'a' | 'b') => {
    const t = Math.min(position, duration)
    if (which === 'a') {
      const end = p.loop_end > t ? p.loop_end : Math.min(duration, t + 8)
      onChange({ loop_start: t, loop_end: end, loop_enabled: true })
    } else {
      const start = p.loop_start < t ? p.loop_start : Math.max(0, t - 8)
      onChange({ loop_start: start, loop_end: t, loop_enabled: true })
    }
  }

  const addMarker = () => {
    const name = `Marcador ${p.markers.length + 1}`
    onChange({ markers: [...p.markers, { id: crypto.randomUUID(), name, time: Math.min(position, duration) }] })
  }

  return (
    <div className="practice">
      <section className="practice-group">
        <header>
          <strong>Velocidad</strong>
          <span className="mono">{pct(p.tempo)}</span>
        </header>
        <div className="stepper">
          <button className="btn small" onClick={() => onChange({ tempo: Math.max(0.25, +(p.tempo - 0.05).toFixed(2)) })}>
            −5%
          </button>
          <input
            type="range"
            min={0.25}
            max={2}
            step={0.01}
            value={p.tempo}
            onChange={(e) => onChange({ tempo: Number(e.target.value) })}
            onDoubleClick={() => onChange({ tempo: 1 })}
            aria-label="Velocidad"
          />
          <button className="btn small" onClick={() => onChange({ tempo: Math.min(2, +(p.tempo + 0.05).toFixed(2)) })}>
            +5%
          </button>
        </div>
        <button className="link small" onClick={() => onChange({ tempo: 1 })} disabled={p.tempo === 1}>
          Original
        </button>
      </section>

      <section className="practice-group">
        <header>
          <strong>Tono</strong>
          <span className="mono">{signed(p.pitch)} st</span>
        </header>
        <div className="stepper">
          <button className="btn small" onClick={() => onChange({ pitch: Math.max(-12, p.pitch - 1) })}>
            −1
          </button>
          <input
            type="range"
            min={-12}
            max={12}
            step={1}
            value={p.pitch}
            onChange={(e) => onChange({ pitch: Number(e.target.value) })}
            onDoubleClick={() => onChange({ pitch: 0 })}
            aria-label="Tono en semitonos"
          />
          <button className="btn small" onClick={() => onChange({ pitch: Math.min(12, p.pitch + 1) })}>
            +1
          </button>
        </div>
        <button className="link small" onClick={() => onChange({ pitch: 0 })} disabled={p.pitch === 0}>
          Original
        </button>
      </section>

      <section className="practice-group">
        <header>
          <strong>Bucle A-B</strong>
          <label className="switch">
            <input
              type="checkbox"
              checked={p.loop_enabled}
              disabled={p.loop_end <= p.loop_start}
              onChange={(e) => onChange({ loop_enabled: e.target.checked })}
            />
            <span />
          </label>
        </header>
        <div className="stepper">
          <button className="btn small" title="Inicio del bucle en la posición actual ([)" onClick={() => setLoopPoint('a')}>
            A
          </button>
          <span className="mono small loop-range">
            {p.loop_end > p.loop_start ? `${formatTime(p.loop_start)} – ${formatTime(p.loop_end)}` : 'sin bucle'}
          </span>
          <button className="btn small" title="Fin del bucle en la posición actual (])" onClick={() => setLoopPoint('b')}>
            B
          </button>
        </div>
        <span className="small muted">También puedes arrastrar sobre la regla.</span>
      </section>

      <section className="practice-group">
        <header>
          <strong>Metrónomo</strong>
          <label className="switch">
            <input type="checkbox" checked={p.metronome} onChange={(e) => onChange({ metronome: e.target.checked })} />
            <span />
          </label>
        </header>
        <div className="stepper">
          <input
            className="num"
            type="number"
            min={20}
            max={400}
            step={0.1}
            value={p.bpm}
            onChange={(e) => onChange({ bpm: Number(e.target.value) || 120 })}
            aria-label="BPM"
          />
          <span className="small muted">BPM</span>
          <button className="btn small" onClick={tap} title="Pulsa al ritmo de la canción">
            Tap
          </button>
          <select
            className="compact"
            value={p.beats_per_bar}
            onChange={(e) => onChange({ beats_per_bar: Number(e.target.value) })}
            aria-label="Pulsos por compás"
          >
            {[2, 3, 4, 5, 6, 7, 8, 12].map((n) => (
              <option key={n} value={n}>
                {n}/4
              </option>
            ))}
          </select>
        </div>
        <div className="stepper">
          <button className="btn small" onClick={() => onChange({ beat_offset: position })} title="El tiempo 1 cae aquí">
            Tiempo 1 aquí
          </button>
          <input
            type="range"
            min={-40}
            max={6}
            step={1}
            value={p.metronome_gain_db}
            onChange={(e) => onChange({ metronome_gain_db: Number(e.target.value) })}
            aria-label="Volumen del metrónomo"
            title={`Volumen ${p.metronome_gain_db} dB`}
          />
        </div>
        <label className="inline small">
          Cuenta previa
          <select
            className="compact"
            value={p.count_in_bars}
            onChange={(e) => onChange({ count_in_bars: Number(e.target.value) })}
          >
            <option value={0}>No</option>
            <option value={1}>1 compás</option>
            <option value={2}>2 compases</option>
          </select>
        </label>
      </section>

      <section className="practice-group">
        <header>
          <strong>Entrenador</strong>
          <label className="switch">
            <input
              type="checkbox"
              checked={p.trainer_enabled}
              disabled={!p.loop_enabled}
              onChange={(e) =>
                onChange(
                  e.target.checked ? { trainer_enabled: true, tempo: p.trainer_start } : { trainer_enabled: false },
                )
              }
            />
            <span />
          </label>
        </header>
        <div className="trainer-grid small">
          <label>
            Desde
            <input
              className="num"
              type="number"
              min={25}
              max={200}
              step={5}
              value={Math.round(p.trainer_start * 100)}
              onChange={(e) => onChange({ trainer_start: Number(e.target.value) / 100 })}
            />
            %
          </label>
          <label>
            Hasta
            <input
              className="num"
              type="number"
              min={25}
              max={200}
              step={5}
              value={Math.round(p.trainer_target * 100)}
              onChange={(e) => onChange({ trainer_target: Number(e.target.value) / 100 })}
            />
            %
          </label>
          <label>
            +
            <input
              className="num"
              type="number"
              min={1}
              max={50}
              value={Math.round(p.trainer_step * 100)}
              onChange={(e) => onChange({ trainer_step: Number(e.target.value) / 100 })}
            />
            % cada
            <input
              className="num"
              type="number"
              min={1}
              max={32}
              value={p.trainer_every}
              onChange={(e) => onChange({ trainer_every: Number(e.target.value) })}
            />
            vueltas
          </label>
        </div>
        {!p.loop_enabled && <span className="small muted">Activa un bucle para usarlo.</span>}
      </section>

      <section className="practice-group markers-group">
        <header>
          <strong>Marcadores</strong>
          <button className="btn small" onClick={addMarker} title="Añadir en la posición actual (M)">
            <Icon name="plus" size={14} /> Añadir
          </button>
        </header>
        <div className="marker-list">
          {p.markers.length === 0 && <span className="small muted">Pulsa M para marcar la posición actual.</span>}
          {p.markers.map((m) => (
            <div key={m.id} className="marker-chip">
              {editing === m.id ? (
                <input
                  autoFocus
                  type="text"
                  defaultValue={m.name}
                  onBlur={(e) => {
                    setEditing(null)
                    const name = e.target.value.trim() || m.name
                    onChange({ markers: p.markers.map((x) => (x.id === m.id ? { ...x, name } : x)) })
                  }}
                  onKeyDown={(e) => e.key === 'Enter' && (e.target as HTMLInputElement).blur()}
                />
              ) : (
                <button className="marker-go" onClick={() => onSeek(m.time)} onDoubleClick={() => setEditing(m.id)}>
                  <span className="mono small">{formatTime(m.time).slice(0, -3)}</span> {m.name}
                </button>
              )}
              <button
                className="marker-del"
                title="Eliminar"
                onClick={() => onChange({ markers: p.markers.filter((x) => x.id !== m.id) })}
              >
                <Icon name="x" size={12} />
              </button>
            </div>
          ))}
        </div>
      </section>
    </div>
  )
}
