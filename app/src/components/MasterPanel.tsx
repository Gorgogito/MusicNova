import { useState } from 'react'
import { open } from '@tauri-apps/plugin-dialog'
import { api, errorMessage, type AutoMasterReport, type LoudnessSummary, type MasterSettings, type TrackView } from '../lib/api'
import type { MasterJob } from '../lib/useMasterReference'
import { Icon } from './Icon'
import { tr } from '../i18n'

interface Props {
  tracks: TrackView[]
  workerReady: boolean
  job: MasterJob | null
  lastWarnings: string[]
  onStartReference: (reference: string) => void
  onCancelReference: () => void
  onMasterChange: (m: MasterSettings) => void
  onToggleSolo: (track: TrackView) => void
  onOpenMix: () => void
  onError: (m: string) => void
}

/** Objetivos habituales de sonoridad integrada. */
const TARGETS = [
  { lufs: -14, label: tr('Streaming: Spotify, YouTube, Tidal (-14 LUFS)') },
  { lufs: -16, label: tr('Apple Music y podcasts (-16 LUFS)') },
  { lufs: -11, label: tr('Moderno, con pegada (-11 LUFS)') },
  { lufs: -9, label: tr('Muy fuerte, estilo club (-9 LUFS)') },
  { lufs: -23, label: tr('Radio y TV, EBU R128 (-23 LUFS)') },
]

const fmt = (v: number | null, unit: string) => (v == null ? '—' : `${v.toFixed(1)} ${unit}`)

function Summary({ title, s }: { title: string; s: LoudnessSummary }) {
  return (
    <div className="master-summary">
      <span className="small muted">{title}</span>
      <strong className="mono">{fmt(s.lufs, 'LUFS')}</strong>
      <span className="small mono">pico real {fmt(s.true_peak_db, 'dBTP')}</span>
      <span className="small mono">LRA {s.lra.toFixed(1)} LU</span>
    </div>
  )
}

export function MasterPanel(props: Props) {
  const { job, workerReady } = props
  const [target, setTarget] = useState(-14)
  const [ceiling, setCeiling] = useState(-1)
  const [busy, setBusy] = useState(false)
  const [report, setReport] = useState<AutoMasterReport | null>(null)
  const [reference, setReference] = useState<string | null>(null)
  const masterTrack = props.tracks.find((t) => t.file.startsWith('audio/master/'))

  const runAuto = async () => {
    setBusy(true)
    try {
      const r = await api.masterAuto(target, ceiling)
      setReport(r)
      props.onMasterChange(r.master)
    } catch (e) {
      props.onError(errorMessage(e))
    } finally {
      setBusy(false)
    }
  }

  const pickReference = async () => {
    const path = await open({
      multiple: false,
      filters: [{ name: tr('Audio'), extensions: ['wav', 'flac', 'mp3', 'ogg', 'aiff', 'aif'] }],
    })
    if (typeof path === 'string') setReference(path)
  }

  return (
    <div className="master">
      <section className="practice-group">
        <header>
          <strong>{tr('Masterización automática')}</strong>
        </header>
        <p className="small muted">
          {tr('Ajusta la sonoridad de la mezcla al objetivo con un limitador de pico real al final del máster. Se conservan los demás efectos del máster (EQ, compresor…) que pongas en Mezcla.')}
        </p>
        <div className="panel-row">
          <label className="field">
            <span className="small muted">{tr('Objetivo')}</span>
            <select value={target} onChange={(e) => setTarget(Number(e.target.value))}>
              {TARGETS.map((t) => (
                <option key={t.lufs} value={t.lufs}>
                  {t.label}
                </option>
              ))}
            </select>
          </label>
          <label className="field">
            <span className="small muted">{tr('Techo de pico real')}</span>
            <select className="compact" value={ceiling} onChange={(e) => setCeiling(Number(e.target.value))}>
              <option value={-1}>{tr('-1 dBTP (recomendado)')}</option>
              <option value={-2}>{tr('-2 dBTP (más margen para MP3/AAC)')}</option>
              <option value={-0.3}>-0,3 dBTP</option>
            </select>
          </label>
          <button className="btn primary" disabled={busy} onClick={runAuto}>
            <Icon name="sliders" size={16} /> {busy ? tr('Ajustando…') : tr('Masterizar')}
          </button>
        </div>
        {busy && (
          <div className="progress indeterminate">
            <div />
          </div>
        )}
        {report && !busy && (
          <>
            <div className="master-compare">
              <Summary title={tr('Antes')} s={report.before} />
              <span className="muted">→</span>
              <Summary title={tr('Después')} s={report.after} />
            </div>
            {!report.reached && (
              <div className="notice small">
                {tr('No se alcanzó el objetivo exacto: haría falta más ganancia de la que el limitador admite sin distorsionar.')}
              </div>
            )}
            {report.after.lra < report.before.lra - 4 && (
              <div className="notice small">
                {tr('La dinámica se ha reducido bastante (LRA {before} → {after} LU). Si suena aplastado, prueba un objetivo menos fuerte.', {
                  before: report.before.lra.toFixed(1),
                  after: report.after.lra.toFixed(1),
                })}
              </div>
            )}
            <button className="link small" onClick={props.onOpenMix}>
              {tr('Ver el limitador y los efectos del máster')}
            </button>
          </>
        )}
      </section>

      <section className="practice-group">
        <header>
          <strong>{tr('Master por referencia')}</strong>
        </header>
        <p className="small muted">
          {tr('Elige una canción que suene como quieres (WAV o FLAC mejor que MP3): se igualan su respuesta en frecuencia, su nivel y su anchura estéreo con Matchering. El resultado se añade como pista nueva.')}
        </p>
        {job ? (
          <>
            <div className="panel-row">
              <span className="muted small">{job.message}</span>
              <button className="btn ghost small" onClick={props.onCancelReference} disabled={!job.jobId}>
                <Icon name="x" size={14} /> {tr('Cancelar')}
              </button>
            </div>
            <div className="progress">
              <div style={{ width: `${job.progress * 100}%` }} />
            </div>
          </>
        ) : (
          <div className="panel-row">
            <button className="btn" onClick={pickReference}>
              <Icon name="folder" size={16} /> {reference ? tr('Cambiar referencia') : tr('Elegir referencia…')}
            </button>
            {reference && <span className="small mono ellipsis" title={reference}>{reference.split(/[\\/]/).pop()}</span>}
            <button
              className="btn primary"
              disabled={!reference || !workerReady}
              onClick={() => reference && props.onStartReference(reference)}
            >
              <Icon name="music" size={16} /> {tr('Masterizar con referencia')}
            </button>
          </div>
        )}
        {props.lastWarnings.map((w) => (
          <div key={w} className="notice small">
            Matchering: {w}
          </div>
        ))}
        {masterTrack && !job && (
          <div className="panel-row">
            <span className="small">
              <strong>{masterTrack.name}</strong>
            </span>
            <div className="seg small">
              <button className={masterTrack.solo ? '' : 'on'} onClick={() => masterTrack.solo && props.onToggleSolo(masterTrack)}>
                {tr('A · Mezcla')}
              </button>
              <button className={masterTrack.solo ? 'on' : ''} onClick={() => !masterTrack.solo && props.onToggleSolo(masterTrack)}>
                {tr('B · Master')}
              </button>
            </div>
            <span className="small muted">
              {tr('Para exportar el master, deja B activo y exporta la mezcla (desactiva antes los efectos del bus máster para no procesarlo dos veces).')}
            </span>
          </div>
        )}
      </section>
    </div>
  )
}
