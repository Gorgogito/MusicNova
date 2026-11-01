import type { EffectSlot, MasterSettings, TrackView } from '../lib/api'
import { EFFECT_LABELS } from '../lib/effects'
import { TRACK_KINDS } from '../lib/tracks'
import { EffectRack } from './EffectRack'

interface Props {
  tracks: TrackView[]
  master: MasterSettings
  /** Canal seleccionado: id de pista o `null` para el máster. */
  selected: string | null
  lufs: { m: number | null; s: number | null }
  onSelect: (id: string | null) => void
  onTrackEffects: (trackId: string, effects: EffectSlot[]) => void
  onMasterEffects: (effects: EffectSlot[]) => void
  onMasterGain: (db: number) => void
}

const LUFS_MIN = -40
/** Niveles a los que normalizan las plataformas de streaming. */
const TARGETS = [
  { lufs: -14, label: 'Spotify / YouTube' },
  { lufs: -16, label: 'Apple Music' },
]

const summary = (effects: EffectSlot[]) =>
  effects.filter((e) => e.enabled).map((e) => EFFECT_LABELS[e.params.type]).join(' · ') || 'sin efectos'

export function MixPanel({ tracks, master, selected, lufs, onSelect, onTrackEffects, onMasterEffects, onMasterGain }: Props) {
  const track = selected ? tracks.find((t) => t.id === selected) : undefined
  const pct = (v: number | null) => (v == null ? 0 : Math.max(0, Math.min(1, (v - LUFS_MIN) / -LUFS_MIN)) * 100)
  const fmt = (v: number | null) => (v == null ? '—' : v.toFixed(1))

  return (
    <div className="mix">
      <aside className="mix-channels">
        <button className={`mix-channel ${selected === null ? 'on' : ''}`} onClick={() => onSelect(null)}>
          <span className="lane-chip" style={{ background: 'var(--accent)' }} />
          <span>
            <strong>Máster</strong>
            <span className="small muted">{summary(master.effects)}</span>
          </span>
        </button>
        {tracks.map((t) => (
          <button key={t.id} className={`mix-channel ${selected === t.id ? 'on' : ''}`} onClick={() => onSelect(t.id)}>
            <span className="lane-chip" style={{ background: TRACK_KINDS[t.kind].color }} />
            <span>
              <strong>{t.name}</strong>
              <span className="small muted">{summary(t.effects)}</span>
            </span>
          </button>
        ))}
      </aside>

      <div className="mix-detail">
        {track ? (
          <>
            <p className="small muted">
              Efectos de <strong>{track.name}</strong>: se aplican antes del volumen y el panorama, y también al exportar.
            </p>
            <EffectRack effects={track.effects} onChange={(fx) => onTrackEffects(track.id, fx)} />
          </>
        ) : (
          <>
            <div className="master-strip">
              <div className="lufs">
                <div className="lufs-values">
                  <span>
                    <span className="small muted">Momentáneo</span>
                    <strong className="mono">{fmt(lufs.m)}</strong>
                  </span>
                  <span>
                    <span className="small muted">Corto plazo (3 s)</span>
                    <strong className="mono">{fmt(lufs.s)}</strong>
                  </span>
                  <span className="small muted">LUFS</span>
                </div>
                <div className="lufs-bar" title="Sonoridad a corto plazo">
                  <div className="lufs-fill" style={{ width: `${pct(lufs.s)}%` }} />
                  <div className="lufs-m" style={{ left: `${pct(lufs.m)}%` }} />
                  {TARGETS.map((t) => (
                    <span key={t.lufs} className="lufs-target" style={{ left: `${pct(t.lufs)}%` }} title={`${t.label}: ${t.lufs} LUFS`}>
                      {t.lufs}
                    </span>
                  ))}
                </div>
              </div>
              <label className="fx-control master-gain">
                <span className="small muted">Ganancia máster</span>
                <input
                  type="range"
                  min={-24}
                  max={12}
                  step={0.5}
                  value={master.gain_db}
                  onChange={(e) => onMasterGain(Number(e.target.value))}
                  onDoubleClick={() => onMasterGain(0)}
                />
                <span className="small mono">{master.gain_db.toFixed(1)} dB</span>
              </label>
            </div>
            <p className="small muted">
              Efectos del máster: se aplican a la mezcla completa después del cambio de tempo y tono. Un limitador al
              final evita la saturación al exportar.
            </p>
            <EffectRack effects={master.effects} onChange={onMasterEffects} />
          </>
        )}
      </div>
    </div>
  )
}
