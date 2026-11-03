import { useCallback, useEffect, useMemo, useState } from 'react'
import { api, errorMessage, type DenoiseSettings, type GeneratedTrack, type ModelInfo, type ProjectView, type TrackView } from '../lib/api'
import { keyName, type Notation } from '../lib/chords'
import { formatTime } from '../lib/format'
import { TRACK_KINDS } from '../lib/tracks'
import { GENERATION_LABEL, type GenerationJobs } from '../lib/useGeneration'
import { Icon } from './Icon'
import { tr } from '../i18n'

interface Props {
  project: ProjectView
  jobs: GenerationJobs
  workerReady: boolean
  notation: Notation
  onProjectReplaced: (p: ProjectView) => void
  onOpenModels: () => void
  onError: (m: string) => void
}

interface TaskInfo {
  id: string
  available: boolean
  unavailable_reason: string
}

const GENERATED_TRACKS: { id: GeneratedTrack; label: string; hint: string }[] = [
  { id: 'drums', label: tr('Batería'), hint: tr('batería acústica, groove rock con charles en corcheas') },
  { id: 'bass', label: tr('Bajo'), hint: tr('bajo eléctrico con dedos, línea sencilla que sigue al bombo') },
  { id: 'guitar', label: tr('Guitarra'), hint: tr('guitarra acústica rasgueada, acordes abiertos') },
  { id: 'keyboard', label: tr('Teclado'), hint: tr('piano eléctrico con acordes suaves') },
  { id: 'strings', label: tr('Cuerdas'), hint: tr('sección de cuerdas cálida, notas largas') },
  { id: 'synth', label: tr('Sintetizador'), hint: tr('pad de sintetizador analógico') },
  { id: 'percussion', label: tr('Percusión'), hint: tr('percusión latina, congas y shaker') },
  { id: 'brass', label: tr('Metales'), hint: tr('sección de metales funk con frases cortas') },
  { id: 'woodwinds', label: tr('Vientos de madera'), hint: tr('flauta y clarinete con melodías suaves') },
  { id: 'backing_vocals', label: tr('Coros'), hint: tr('coros armonizados «uuh» y «aah»') },
  { id: 'vocals', label: tr('Voz'), hint: tr('voz principal melódica') },
  { id: 'fx', label: tr('Efectos'), hint: tr('efectos ambientales y transiciones') },
]

const QUALITY = [
  { steps: 16, label: tr('Rápida (16 pasos)') },
  { steps: 32, label: tr('Normal (32 pasos)') },
  { steps: 50, label: tr('Alta (50 pasos)') },
]

/** Pistas que se pueden limpiar: todas salvo las generadas por esta fase. */
const cleanable = (t: TrackView) => !t.file.startsWith('audio/denoised/') && !t.file.startsWith('audio/generated/')
const isVoice = (t: TrackView) => t.kind === 'vocals' || t.kind === 'backing_vocals' || t.kind === 'recording'

export function GenerationPanel(props: Props) {
  const { jobs, workerReady } = props
  const job = jobs.job
  const [tasks, setTasks] = useState<TaskInfo[] | null>(null)
  const [models, setModels] = useState<ModelInfo[] | null>(null)
  // Reloj del tiempo transcurrido mientras hay un trabajo.
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (!job) return
    const id = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(id)
  }, [job])

  const refresh = useCallback(() => {
    if (!workerReady) return
    Promise.all([api.worker<TaskInfo[]>('tasks.list'), api.worker<{ models: ModelInfo[] }>('models.list')])
      .then(([t, m]) => {
        setTasks(t)
        setModels(m.models)
      })
      .catch((e) => props.onError(errorMessage(e)))
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workerReady])
  useEffect(refresh, [refresh])
  // Al terminar una instalación cambia la disponibilidad.
  useEffect(() => {
    if (!job) refresh()
  }, [job, refresh])

  if (job) {
    const elapsed = Math.max(0, (now - job.startedAt) / 1000)
    return (
      <div className="panel">
        <div className="panel-row">
          <strong>{GENERATION_LABEL[job.kind]}</strong>
          <span className="muted small">
            {job.message} · {Math.floor(elapsed / 60)}:{String(Math.floor(elapsed % 60)).padStart(2, '0')}
          </span>
          <button className="btn ghost small" onClick={jobs.cancel} disabled={!job.jobId}>
            <Icon name="x" size={14} /> {tr('Cancelar')}
          </button>
        </div>
        <div className="progress">
          <div style={{ width: `${job.progress * 100}%` }} />
        </div>
        {job.kind === 'generate' && (
          <span className="small muted">{tr('ACE-Step tarda varios minutos por pista en una GPU de 8 GB; puedes seguir usando la aplicación.')}</span>
        )}
      </div>
    )
  }

  return (
    <div className="panel transcription-panel">
      {!workerReady && <span className="muted">{tr('El proceso de IA se está iniciando…')}</span>}
      <DenoiseSection {...props} models={models} />
      <GenerateSection {...props} tasks={tasks} models={models} />
    </div>
  )
}

// ---- Limpieza de ruido ------------------------------------------------------------------

function DenoiseSection(props: Props & { models: ModelInfo[] | null }) {
  const { project, jobs, workerReady, models } = props
  const tracks = useMemo(() => project.tracks.filter(cleanable), [project.tracks])
  const [trackId, setTrackId] = useState(() => (tracks.find(isVoice) ?? tracks[0])?.id ?? '')
  const track = tracks.find((t) => t.id === trackId) ?? tracks[0]
  const [mode, setMode] = useState<'voice' | 'steady'>(track && isVoice(track) ? 'voice' : 'steady')
  const [voiceLimit, setVoiceLimit] = useState(40)
  const [steady, setSteady] = useState<Omit<DenoiseSettings, 'noise_region'>>({ reduction_db: 18, sensitivity: 0.3 })
  const practice = project.project.practice
  const hasLoop = practice.loop_end > practice.loop_start + 0.2
  const [useLoop, setUseLoop] = useState(false)
  const [busy, setBusy] = useState(false)
  const [status, setStatus] = useState<string | null>(null)
  const dfnInstalled = models?.some((m) => m.id === 'deepfilternet3' && m.installed) ?? false

  const run = async () => {
    if (!track) return
    setStatus(null)
    if (mode === 'voice') {
      // 40 dB en el control = sin límite de atenuación.
      jobs.cleanVoice(track.id, voiceLimit >= 40 ? null : voiceLimit)
      return
    }
    setBusy(true)
    try {
      const region: [number, number] | null = useLoop && hasLoop ? [practice.loop_start, practice.loop_end] : null
      props.onProjectReplaced(await api.denoiseSteady(track.id, { ...steady, noise_region: region }))
      setStatus(tr('Se añadió la pista sin ruido; la original queda silenciada para comparar (M).'))
    } catch (e) {
      props.onError(errorMessage(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <section className="tx-section">
      <h3>
        <Icon name="wave" size={16} /> {tr('Limpiar ruido')}
      </h3>
      <p className="small muted">
        {tr('Crea una versión limpia de una pista; la original queda silenciada para comparar. «Voz» usa una red neuronal (DeepFilterNet3) que separa la voz de cualquier ruido de fondo. «Ruido constante» quita zumbidos, siseo o ventiladores de cualquier instrumento sin tocar el resto.')}
      </p>
      {tracks.length === 0 ? (
        <span className="small muted">{tr('No hay pistas que limpiar.')}</span>
      ) : (
        <>
          <div className="panel-row">
            <label className="field">
              <span className="small muted">{tr('Pista')}</span>
              <select
                value={track?.id ?? ''}
                onChange={(e) => {
                  setTrackId(e.target.value)
                  const t = tracks.find((x) => x.id === e.target.value)
                  if (t) setMode(isVoice(t) ? 'voice' : 'steady')
                }}
              >
                {tracks.map((t) => (
                  <option key={t.id} value={t.id}>
                    {t.name} ({TRACK_KINDS[t.kind].label})
                  </option>
                ))}
              </select>
            </label>
            <label className="field">
              <span className="small muted">{tr('Tipo')}</span>
              <select value={mode} onChange={(e) => setMode(e.target.value as typeof mode)}>
                <option value="voice">{tr('Voz o habla (IA)')}</option>
                <option value="steady">{tr('Ruido constante (cualquier instrumento)')}</option>
              </select>
            </label>
            {mode === 'voice' ? (
              <label className="field" title={tr('Cuánto puede bajar el ruido. Un límite deja algo de ambiente y suena más natural')}>
                <span className="small muted">{tr('Atenuación máxima:')} {voiceLimit >= 40 ? tr('sin límite') : `${voiceLimit} dB`}</span>
                <input type="range" min={6} max={40} step={1} value={voiceLimit} onChange={(e) => setVoiceLimit(Number(e.target.value))} />
              </label>
            ) : (
              <>
                <label className="field" title={tr('Cuánto se atenúa el ruido como máximo')}>
                  <span className="small muted">{tr('Reducción')} {steady.reduction_db} dB</span>
                  <input
                    type="range"
                    min={6}
                    max={36}
                    step={1}
                    value={steady.reduction_db}
                    onChange={(e) => setSteady({ ...steady, reduction_db: Number(e.target.value) })}
                  />
                </label>
                <label className="field" title={tr('Más alta quita más ruido, pero también algo de los sonidos más débiles')}>
                  <span className="small muted">{tr('Sensibilidad')} {Math.round(steady.sensitivity * 100)} %</span>
                  <input
                    type="range"
                    min={0}
                    max={1}
                    step={0.05}
                    value={steady.sensitivity}
                    onChange={(e) => setSteady({ ...steady, sensitivity: Number(e.target.value) })}
                  />
                </label>
                <label
                  className="check small"
                  title={hasLoop ? tr('Marca un bucle A-B sobre un tramo donde solo se oiga el ruido') : tr('Marca un bucle A-B (en Práctica) sobre un tramo con solo ruido')}
                >
                  <input type="checkbox" checked={useLoop && hasLoop} disabled={!hasLoop} onChange={(e) => setUseLoop(e.target.checked)} />
                  {tr('Tomar el ruido del bucle A-B')}
                  {hasLoop && ` (${formatTime(practice.loop_start)}–${formatTime(practice.loop_end)})`}
                </label>
              </>
            )}
            {mode === 'voice' && models !== null && !dfnInstalled ? (
              <button className="btn primary" onClick={props.onOpenModels}>
                <Icon name="download" size={16} /> {tr('Descargar DeepFilterNet3 (8 MB)')}
              </button>
            ) : (
              <button className="btn primary" disabled={busy || !track || (mode === 'voice' && !workerReady)} onClick={run}>
                <Icon name="star" size={16} /> {busy ? tr('Limpiando…') : tr('Limpiar')}
              </button>
            )}
          </div>
          {mode === 'steady' && !useLoop && (
            <span className="small muted">
              {tr('Sin bucle, el ruido se mide en los momentos más silenciosos de la pista (la cuenta previa, las pausas): si la pista no tiene silencios, marca un tramo con solo ruido.')}
            </span>
          )}
        </>
      )}
      {status && <div className="small muted">{status}</div>}
    </section>
  )
}

// ---- Acompañamientos ----------------------------------------------------------------------

function GenerateSection(props: Props & { tasks: TaskInfo[] | null; models: ModelInfo[] | null }) {
  const { project, jobs, workerReady, tasks, models, notation } = props
  const [track, setTrack] = useState<GeneratedTrack>('drums')
  const [caption, setCaption] = useState('')
  const [context, setContext] = useState<'audible' | 'original'>('audible')
  const [steps, setSteps] = useState(32)
  const [seed, setSeed] = useState('')
  const envReady = tasks?.find((t) => t.id === 'generate_stem')?.available ?? false
  const model = models?.find((m) => m.id === 'acestep_v15_base')
  const analysis = project.analysis
  const info = GENERATED_TRACKS.find((t) => t.id === track)!
  const muted = project.tracks.filter((t) => t.mute).map((t) => t.name)

  return (
    <section className="tx-section">
      <h3>
        <Icon name="music" size={16} /> {tr('Generar acompañamiento')}
      </h3>
      <p className="small muted">
        {tr('Crea una pista nueva que encaja con la canción (tempo, tonalidad y estructura) con ACE-Step 1.5 (MIT). Para sustituir una parte, silencia su stem y genera esa pista con «lo que suena ahora» como contexto.')}
      </p>
      {tasks === null || models === null ? (
        <span className="small muted">{tr('Comprobando ACE-Step…')}</span>
      ) : !envReady ? (
        <div className="panel-row">
          <span className="small">
            {tr('Paso 1: instalar el entorno de ACE-Step (Python aparte con PyTorch 2.7 para CUDA, unos 3–4 GB). Solo se hace una vez.')}
          </span>
          <button className="btn primary" disabled={!workerReady} onClick={jobs.installAceStep}>
            <Icon name="download" size={16} /> {tr('Instalar entorno')}
          </button>
        </div>
      ) : !model?.installed ? (
        <div className="panel-row">
          <span className="small">{tr('Paso 2: descargar el modelo ACE-Step 1.5 base (6,3 GB).')}</span>
          <button className="btn primary" onClick={props.onOpenModels}>
            <Icon name="download" size={16} /> {tr('Ver modelos')}
          </button>
        </div>
      ) : (
        <>
          <div className="panel-row">
            <label className="field">
              <span className="small muted">{tr('Pista a generar')}</span>
              <select value={track} onChange={(e) => setTrack(e.target.value as GeneratedTrack)}>
                {GENERATED_TRACKS.map((t) => (
                  <option key={t.id} value={t.id}>
                    {t.label}
                  </option>
                ))}
              </select>
            </label>
            <label className="field grow" title={tr('Describe el estilo de la pista (en inglés suele funcionar mejor)')}>
              <span className="small muted">{tr('Estilo (opcional)')}</span>
              <input type="text" value={caption} maxLength={512} placeholder={info.hint} onChange={(e) => setCaption(e.target.value)} />
            </label>
          </div>
          <div className="panel-row">
            <label className="field">
              <span className="small muted">{tr('Contexto')}</span>
              <select value={context} onChange={(e) => setContext(e.target.value as typeof context)}>
                <option value="audible">{muted.length ? tr('Lo que suena ahora (sin {n} silenciadas)', { n: muted.length }) : tr('Lo que suena ahora')}</option>
                <option value="original">{tr('Mezcla original')}</option>
              </select>
            </label>
            <label className="field">
              <span className="small muted">{tr('Calidad')}</span>
              <select value={steps} onChange={(e) => setSteps(Number(e.target.value))}>
                {QUALITY.map((q) => (
                  <option key={q.steps} value={q.steps}>
                    {q.label}
                  </option>
                ))}
              </select>
            </label>
            <label className="field" title={tr('La misma semilla repite el mismo resultado; vacía, una distinta cada vez')}>
              <span className="small muted">{tr('Semilla')}</span>
              <input type="number" min={0} value={seed} placeholder="aleatoria" onChange={(e) => setSeed(e.target.value)} style={{ width: 110 }} />
            </label>
            <button
              className="btn primary"
              disabled={!workerReady}
              onClick={() => jobs.generate(track, caption.trim() || info.hint, context, steps, seed === '' ? null : Math.max(0, Math.floor(Number(seed))))}
            >
              <Icon name="star" size={16} /> {tr('Generar')} {info.label.toLowerCase()}
            </button>
          </div>
          <span className="small muted">
            {analysis
              ? tr('Se usarán {bpm} BPM, {meter}/4 y {key} del análisis.', { bpm: Math.round(analysis.bpm), meter: analysis.beats_per_bar, key: keyName(analysis.key.tonic, analysis.key.mode as 'major' | 'minor', notation) })
              : tr('Analiza la canción antes para darle a ACE-Step el tempo y la tonalidad.')}{' '}
            {tr('Cada generación se añade como pista nueva; puedes generar varias y quedarte con la mejor.')}
          </span>
        </>
      )}
    </section>
  )
}
