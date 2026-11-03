import { useEffect, useMemo, useState } from 'react'
import { save } from '@tauri-apps/plugin-dialog'
import {
  api,
  errorMessage,
  type InstrumentReport,
  type ModelInfo,
  type MusicalKey,
  type NotesMode,
  type PitchCurve,
  type PitchEngine,
  type ProjectView,
  type TrackView,
  type Transcription,
  type TuneSettings,
} from '../lib/api'
import { keyName, type Notation } from '../lib/chords'
import { formatTime } from '../lib/format'
import { pitchLabel, scaleOf, tuningStats } from '../lib/notes'
import { TRACK_KINDS } from '../lib/tracks'
import { JOB_LABEL, type TranscriptionJobs } from '../lib/useTranscription'
import { Icon } from './Icon'
import { tr, plural, trBackend } from '../i18n'

interface Props {
  project: ProjectView
  jobs: TranscriptionJobs
  workerReady: boolean
  notation: Notation
  /** Pistas con su piano roll a la vista. */
  shownNotes: Set<string>
  onToggleNotes: (trackId: string) => void
  onNotesChange: (t: Transcription) => void
  onProjectReplaced: (p: ProjectView) => void
  onSeek: (t: number) => void
  onOpenModels: () => void
  onError: (m: string) => void
}

/** Pistas que tiene sentido transcribir: todas menos las generadas. */
const transcribable = (t: TrackView) => t.kind !== 'generated'
const isVoice = (t: TrackView) => t.kind === 'vocals' || t.kind === 'backing_vocals' || t.kind === 'recording'
/** Pista por defecto: un stem melódico antes que la mezcla. */
const PREFERRED = ['piano', 'bass', 'guitar', 'vocals', 'other', 'drums', 'recording', 'backing_vocals', 'mix']

export function TranscriptionPanel(props: Props) {
  const { project, jobs, workerReady } = props
  const job = jobs.job
  const tracks = project.tracks
  const candidates = useMemo(
    () => tracks.filter(transcribable).sort((a, b) => PREFERRED.indexOf(a.kind) - PREFERRED.indexOf(b.kind)),
    [tracks],
  )
  const voices = useMemo(() => tracks.filter(isVoice).sort((a) => (a.kind === 'vocals' ? -1 : 1)), [tracks])

  if (job) {
    const track = tracks.find((t) => t.id === job.trackId)
    return (
      <div className="panel">
        <div className="panel-row">
          <strong>
            {JOB_LABEL[job.kind]}
            {track ? ` · ${track.name}` : ''}
          </strong>
          <span className="muted small">{job.message}</span>
          <button className="btn ghost small" onClick={jobs.cancel} disabled={!job.jobId}>
            <Icon name="x" size={14} /> {tr('Cancelar')}
          </button>
        </div>
        <div className="progress">
          <div style={{ width: `${job.progress * 100}%` }} />
        </div>
      </div>
    )
  }

  return (
    <div className="panel transcription-panel">
      {!workerReady && <span className="muted">{tr('El proceso de IA se está iniciando…')}</span>}
      <NotesSection {...props} candidates={candidates} />
      <TuningSection {...props} voices={voices} />
      <InstrumentsSection {...props} />
    </div>
  )
}

// ---- Audio a MIDI -------------------------------------------------------------------

function NotesSection(props: Props & { candidates: TrackView[] }) {
  const { project, candidates, jobs, workerReady, notation } = props
  const [trackId, setTrackId] = useState(candidates[0]?.id ?? '')
  const track = candidates.find((t) => t.id === trackId) ?? candidates[0]
  const [modeChoice, setModeChoice] = useState<'auto' | NotesMode>('auto')
  const [sensitivity, setSensitivity] = useState(0.5)
  const [busy, setBusy] = useState<string | null>(null)
  const [status, setStatus] = useState<string | null>(null)
  const autoMode: NotesMode = !track ? 'poly' : track.kind === 'drums' ? 'drums' : isVoice(track) || track.kind === 'bass' ? 'mono' : 'poly'
  const mode = modeChoice === 'auto' ? autoMode : modeChoice
  const transcribed = project.notes?.tracks.filter((tn) => tracks(project).has(tn.track_id)) ?? []

  const exportMidi = async (only: string | null) => {
    try {
      const name = only ? `${project.project.name} - ${project.tracks.find((t) => t.id === only)?.name}` : project.project.name
      const path = await save({ defaultPath: `${name}.mid`, filters: [{ name: 'MIDI', extensions: ['mid'] }] })
      if (!path) return
      setStatus(tr('Guardado en {path}', { path: await api.notesExportMidi(path, only) }))
    } catch (e) {
      props.onError(errorMessage(e))
    }
  }

  const render = async (id: string) => {
    setBusy(id)
    try {
      props.onProjectReplaced(await api.notesRender(id))
      setStatus(tr('Se añadió una pista con la síntesis de las notas.'))
    } catch (e) {
      props.onError(errorMessage(e))
    } finally {
      setBusy(null)
    }
  }

  const remove = async (id: string) => {
    try {
      props.onNotesChange(await api.notesRemove(id))
    } catch (e) {
      props.onError(errorMessage(e))
    }
  }

  return (
    <section className="tx-section">
      <h3>
        <Icon name="music" size={16} /> {tr('Audio a MIDI')}
      </h3>
      <p className="small muted">
        {tr('Convierte una pista en notas (Basic Pitch) para verlas en un piano roll, corregirlas y exportarlas a MIDI con el tempo de la canción, incluidas las curvas de tono de cada nota. La batería se transcribe como bombo, caja y charles. Funciona mejor con stems separados que con la mezcla.')}
      </p>
      {candidates.length === 0 ? (
        <span className="small muted">{tr('No hay pistas con notas que transcribir.')}</span>
      ) : (
        <div className="panel-row">
          <label className="field">
            <span className="small muted">{tr('Pista')}</span>
            <select value={track?.id ?? ''} onChange={(e) => setTrackId(e.target.value)}>
              {candidates.map((t) => (
                <option key={t.id} value={t.id}>
                  {t.name} ({TRACK_KINDS[t.kind].label})
                </option>
              ))}
            </select>
          </label>
          <label className="field">
            <span className="small muted">{tr('Tipo')}</span>
            <select value={modeChoice} onChange={(e) => setModeChoice(e.target.value as typeof modeChoice)}>
              <option value="auto">{tr('Automático ({mode})', { mode: MODE_LABEL[autoMode] })}</option>
              <option value="poly">{tr('Polifónico (acordes)')}</option>
              <option value="mono">{tr('Monofónico (melodía, bajo, voz)')}</option>
              <option value="drums">{tr('Batería (bombo, caja, charles)')}</option>
            </select>
          </label>
          <label className="field" title={tr('Más sensibilidad detecta notas más débiles, pero también más notas falsas')}>
            <span className="small muted">{tr('Sensibilidad')} {Math.round(sensitivity * 100)} %</span>
            <input type="range" min={0} max={1} step={0.05} value={sensitivity} onChange={(e) => setSensitivity(Number(e.target.value))} />
          </label>
          <button
            className="btn primary"
            disabled={!workerReady || !track}
            onClick={() => track && jobs.startNotes(track.id, mode, sensitivity, 'auto')}
          >
            <Icon name="play" size={16} /> {tr('Transcribir')}
          </button>
        </div>
      )}
      {transcribed.length > 0 && (
        <div className="tx-list">
          {transcribed.map((tn) => {
            const t = project.tracks.find((x) => x.id === tn.track_id)!
            const pitches = tn.notes.map((n) => n.pitch)
            return (
              <div key={tn.track_id} className="tx-item">
                <span className="lane-chip" style={{ background: TRACK_KINDS[t.kind].color }} />
                <strong className="ellipsis">{t.name}</strong>
                <span className="small muted">
                  {plural(tn.notes.length, '{n} nota', '{n} notas')}
                  {tn.mode === 'drums'
                    ? ` · ${[...new Set(pitches)].sort((a, b) => a - b).map((p) => pitchLabel(p, notation, true).toLowerCase()).join(', ')}`
                    : pitches.length > 0 &&
                      ` · ${pitchLabel(Math.min(...pitches), notation, false)}–${pitchLabel(Math.max(...pitches), notation, false)}`}
                  {tn.mode === 'mono' ? tr(' · monofónico') : ''}
                  {tn.notes.some((n) => n.bends?.length) ? ' · ' + tr('{n} con curva', { n: tn.notes.filter((n) => n.bends?.length).length }) : ''}
                </span>
                <div className="tx-actions">
                  <button
                    className={`btn ghost small ${props.shownNotes.has(t.id) ? 'active' : ''}`}
                    onClick={() => props.onToggleNotes(t.id)}
                    title={tr('Mostrar u ocultar el piano roll bajo la pista')}
                  >
                    {tr('Piano roll')}
                  </button>
                  <button className="btn ghost small" onClick={() => render(t.id)} disabled={busy !== null} title={tr('Añade una pista que toca estas notas con un sintetizador')}>
                    {busy === t.id ? tr('Sintetizando…') : tr('Escuchar')}
                  </button>
                  <button className="btn ghost small" onClick={() => exportMidi(t.id)}>
                    <Icon name="download" size={14} /> MIDI
                  </button>
                  <button className="icon-btn" onClick={() => remove(t.id)} title={tr('Borrar estas notas')}>
                    <Icon name="trash" size={14} />
                  </button>
                </div>
              </div>
            )
          })}
          {transcribed.length > 1 && (
            <button className="btn small" onClick={() => exportMidi(null)}>
              <Icon name="download" size={14} /> {tr('Exportar todas a un MIDI')}
            </button>
          )}
        </div>
      )}
      {status && <div className="small muted">{status}</div>}
    </section>
  )
}

const tracks = (p: ProjectView) => new Set(p.tracks.map((t) => t.id))

const MODE_LABEL: Record<NotesMode, string> = { poly: tr('acordes'), mono: tr('una nota a la vez'), drums: tr('batería') }

// ---- Afinación vocal ------------------------------------------------------------------

type ScaleChoice = 'key' | 'chromatic' | `${number}-${'major' | 'minor'}`

function scaleFor(choice: ScaleChoice, key: MusicalKey | undefined): number[] {
  if (choice === 'chromatic') return []
  if (choice === 'key') return key ? scaleOf(key) : []
  const [tonic, mode] = choice.split('-')
  return scaleOf({ tonic: Number(tonic), mode: mode as 'major' | 'minor' })
}

function sourceLabel(source: string): string {
  if (source === 'crepe-full') return 'CREPE'
  if (source === 'crepe-tiny') return tr('CREPE rápido')
  return source === 'pyin' ? 'pYIN' : source
}

function TuningSection(props: Props & { voices: TrackView[] }) {
  const { project, voices, jobs, workerReady, notation } = props
  const curve: PitchCurve | null = project.pitch
  const curveTrack = curve ? project.tracks.find((t) => t.id === curve.track_id) : undefined
  const [trackId, setTrackId] = useState(curveTrack?.id ?? voices[0]?.id ?? '')
  const key = project.analysis?.key
  const [scaleChoice, setScaleChoice] = useState<ScaleChoice>(key ? 'key' : 'chromatic')
  const scale = scaleFor(scaleChoice, key)
  const stats = useMemo(() => (curve ? tuningStats(curve, scale) : null), [curve, scale])
  const [settings, setSettings] = useState<Omit<TuneSettings, 'scale'>>({ strength: 0.8, speed: 0.2, preserve_formants: true })
  const [engine, setEngine] = useState<PitchEngine>('crepe-full')
  const [busy, setBusy] = useState(false)
  const [status, setStatus] = useState<string | null>(null)
  const pct = (x: number) => `${Math.round(x * 100)} %`

  const renderTuned = async () => {
    setBusy(true)
    try {
      props.onProjectReplaced(await api.tuneRender({ ...settings, scale }))
      setStatus(tr('Se añadió la voz afinada; la original queda silenciada para comparar (M).'))
    } catch (e) {
      props.onError(errorMessage(e))
    } finally {
      setBusy(false)
    }
  }

  const keyOptions: { value: ScaleChoice; label: string }[] = []
  for (let tonic = 0; tonic < 12; tonic++) {
    for (const mode of ['major', 'minor'] as const) keyOptions.push({ value: `${tonic}-${mode}`, label: keyName(tonic, mode, notation) })
  }

  return (
    <section className="tx-section">
      <h3>
        <Icon name="mic" size={16} /> {tr('Afinación vocal')}
      </h3>
      <p className="small muted">
        {tr('Sigue el tono de la voz, mide cuánto se aleja de las notas de la tonalidad y crea una versión afinada que conserva el timbre (y, si quieres, el vibrato).')}
      </p>
      {voices.length === 0 ? (
        <span className="small muted">{tr('Separa los stems o graba una toma para tener una pista de voz.')}</span>
      ) : (
        <div className="panel-row">
          <label className="field">
            <span className="small muted">{tr('Pista de voz')}</span>
            <select value={trackId} onChange={(e) => setTrackId(e.target.value)}>
              {voices.map((t) => (
                <option key={t.id} value={t.id}>
                  {t.name}
                </option>
              ))}
            </select>
          </label>
          <label className="field" title={tr('CREPE es una red neuronal: más precisa y distingue mejor la voz de las respiraciones. pYIN es estadístico')}>
            <span className="small muted">{tr('Detector de tono')}</span>
            <select value={engine} onChange={(e) => setEngine(e.target.value as PitchEngine)}>
              <option value="crepe-full">{tr('CREPE (preciso)')}</option>
              <option value="crepe-tiny">{tr('CREPE rápido')}</option>
              <option value="pyin">{tr('pYIN (sin red neuronal)')}</option>
            </select>
          </label>
          <button className="btn primary" disabled={!workerReady || !trackId} onClick={() => jobs.startPitch(trackId, engine)}>
            <Icon name="play" size={16} /> {curve ? tr('Volver a analizar') : tr('Analizar afinación')}
          </button>
        </div>
      )}
      {curve && stats && curveTrack && (
        <>
          <div className="panel-row">
            <label className="field">
              <span className="small muted">{tr('Escala de referencia')}</span>
              <select value={scaleChoice} onChange={(e) => setScaleChoice(e.target.value as ScaleChoice)}>
                {key && <option value="key">Tonalidad detectada ({keyName(key.tonic, key.mode, notation)})</option>}
                <option value="chromatic">{tr('Cromática (cualquier semitono)')}</option>
                {keyOptions.map((o) => (
                  <option key={o.value} value={o.value}>
                    {o.label}
                  </option>
                ))}
              </select>
            </label>
            <div className="tune-stats">
              <span>
                <strong>{pct(stats.inTune)}</strong> <span className="small muted">{tr('afinado (±25 cents)')}</span>
              </span>
              <span>
                <strong>{pct(stats.sharp)}</strong> <span className="small muted">{tr('alto')}</span>
              </span>
              <span>
                <strong>{pct(stats.flat)}</strong> <span className="small muted">{tr('bajo')}</span>
              </span>
              <span>
                <strong>{stats.meanCents.toFixed(0)}</strong> <span className="small muted">{tr('cents de desviación media')}</span>
              </span>
              <span className="small muted">
                {curveTrack.name} · {formatTime(stats.voicedSeconds)} cantados · {sourceLabel(curve.source)}
              </span>
            </div>
          </div>
          <div className="panel-row">
            <label className="field" title={tr('Cuánto se acerca la voz a la nota correcta')}>
              <span className="small muted">{tr('Intensidad')} {pct(settings.strength)}</span>
              <input
                type="range"
                min={0}
                max={1}
                step={0.05}
                value={settings.strength}
                onChange={(e) => setSettings({ ...settings, strength: Number(e.target.value) })}
              />
            </label>
            <label className="field" title={tr('Natural: solo centra cada nota y conserva el vibrato. Robótica: clava la voz en la nota (efecto «auto-tune»)')}>
              <span className="small muted">
                {tr('Estilo:')} {settings.speed < 0.15 ? tr('natural') : settings.speed > 0.85 ? tr('robótico') : tr('intermedio')}
              </span>
              <input
                type="range"
                min={0}
                max={1}
                step={0.05}
                value={settings.speed}
                onChange={(e) => setSettings({ ...settings, speed: Number(e.target.value) })}
              />
            </label>
            <label className="check small">
              <input
                type="checkbox"
                checked={settings.preserve_formants}
                onChange={(e) => setSettings({ ...settings, preserve_formants: e.target.checked })}
              />
              Conservar el timbre (formantes)
            </label>
            <button className="btn primary" disabled={busy} onClick={renderTuned}>
              <Icon name="star" size={16} /> {busy ? tr('Afinando…') : tr('Crear voz afinada')}
            </button>
            <button
              className={`btn ghost small ${props.shownNotes.has(curveTrack.id) ? 'active' : ''}`}
              onClick={() => props.onToggleNotes(curveTrack.id)}
              title={tr('Muestra la curva de tono bajo la pista de voz')}
            >
              {tr('Ver curva')}
            </button>
          </div>
        </>
      )}
      {status && <div className="small muted">{status}</div>}
    </section>
  )
}

// ---- Instrumentos ---------------------------------------------------------------------

function InstrumentsSection(props: Props) {
  const { project, jobs, workerReady } = props
  const report: InstrumentReport | null = project.instruments
  const [models, setModels] = useState<ModelInfo[] | null>(null)
  const [modelId, setModelId] = useState('')

  useEffect(() => {
    if (!workerReady) return
    api
      .worker<{ models: ModelInfo[] }>('models.list')
      .then((r) => {
        const installed = r.models
          .filter((m) => m.task === 'instruments' && m.installed)
          .sort((a, b) => Number(b.priority ?? 0) - Number(a.priority ?? 0))
        setModels(installed)
        setModelId((cur) => (installed.some((m) => m.id === cur) ? cur : (installed[0]?.id ?? '')))
      })
      .catch((e) => props.onError(errorMessage(e)))
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workerReady])

  const duration = report?.duration || 1
  return (
    <section className="tx-section">
      <h3>
        <Icon name="model" size={16} /> {tr('Instrumentos')}
      </h3>
      <p className="small muted">
        {tr('Reconoce qué instrumentos suenan en la canción y en qué momentos (PANNs, clases de AudioSet). Si has separado los stems, busca cada instrumento en el suyo y acierta bastante más.')}
        {!project.tracks.some((t) => t.kind === 'vocals' || t.kind === 'other') && tr(' Este proyecto aún no está separado.')}
      </p>
      {models !== null && models.length === 0 ? (
        <div className="panel-row">
          <span className="small">{tr('Necesitas descargar un modelo PANNs (una sola vez).')}</span>
          <button className="btn primary" onClick={props.onOpenModels}>
            <Icon name="download" size={16} /> {tr('Ver modelos')}
          </button>
        </div>
      ) : (
        <div className="panel-row">
          {models && models.length > 1 && (
            <label className="field">
              <span className="small muted">{tr('Modelo')}</span>
              <select value={modelId} onChange={(e) => setModelId(e.target.value)}>
                {models.map((m) => (
                  <option key={m.id} value={m.id}>
                    {trBackend(m.name)}
                  </option>
                ))}
              </select>
            </label>
          )}
          <button className="btn primary" disabled={!workerReady || !modelId} onClick={() => jobs.startInstruments(modelId || undefined, 'auto')}>
            <Icon name="play" size={16} /> {report ? tr('Volver a detectar') : tr('Detectar instrumentos')}
          </button>
        </div>
      )}
      {report && (
        <span className="small muted">
          {report.stems?.length ? tr('Analizado con los stems separados ({n}).', { n: report.stems.length }) : tr('Analizado con la mezcla.')}
        </span>
      )}
      {report && report.instruments.length === 0 && <span className="small muted">{tr('No se reconoció ningún instrumento con claridad.')}</span>}
      {report && report.instruments.length > 0 && (
        <div className="instr-list">
          {report.instruments.map((i) => (
            <div key={i.id} className="instr-row">
              <span className="instr-name">{trBackend(i.label)}</span>
              <span className="instr-conf" title={`Confianza ${Math.round(i.confidence * 100)} %`}>
                <span style={{ width: `${Math.round(i.confidence * 100)}%` }} />
              </span>
              <span className="small mono muted">{Math.round(i.coverage * 100)} %</span>
              <div className="instr-timeline" title={tr('Cuándo suena (clic: ir a ese momento)')}>
                {i.segments.map(([a, b]) => (
                  <button
                    key={a}
                    className="instr-seg"
                    style={{ left: `${(a / duration) * 100}%`, width: `${((b - a) / duration) * 100}%` }}
                    onClick={() => props.onSeek(a)}
                    title={`${formatTime(a)} – ${formatTime(b)}`}
                  />
                ))}
              </div>
            </div>
          ))}
        </div>
      )}
    </section>
  )
}
