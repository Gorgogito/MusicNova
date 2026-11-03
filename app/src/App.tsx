import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import {
  api,
  errorMessage,
  events,
  timelineEnd,
  type Analysis,
  type AppInfo,
  type AutomationLane,
  type Clip,
  type InstrumentReport,
  type Lyrics,
  type MasterReferenceResult,
  type MasterSettings,
  type PitchCurve,
  type PracticeSettings,
  type ProjectView,
  type TrackInfo,
  type Transcription,
  type TransportEvent,
  type WorkerStatus,
} from './lib/api'
import { Icon } from './components/Icon'
import { TransportBar } from './components/TransportBar'
import { ProjectsView } from './views/ProjectsView'
import { StudioView } from './views/StudioView'
import { ModelsView } from './views/ModelsView'
import { SystemView } from './views/SystemView'
import { useSeparation } from './lib/useSeparation'
import { useAnalysis } from './lib/useAnalysis'
import { useLyrics } from './lib/useLyrics'
import { JOB_LABEL, useTranscription } from './lib/useTranscription'
import { GENERATION_LABEL, useGeneration } from './lib/useGeneration'
import { useMasterReference } from './lib/useMasterReference'
import { useRecorder } from './lib/useRecorder'
import { tr, trBackend } from './i18n'

type View = 'projects' | 'studio' | 'models' | 'system'

const NAV: { id: View; label: string; icon: string }[] = [
  { id: 'projects', label: tr('Proyectos'), icon: 'folder' },
  { id: 'studio', label: tr('Estudio'), icon: 'studio' },
  { id: 'models', label: tr('Modelos IA'), icon: 'model' },
  { id: 'system', label: tr('Sistema'), icon: 'system' },
]

const IDLE: TransportEvent = {
  position: 0,
  duration: 0,
  playing: false,
  counting_in: false,
  loop_count: 0,
  peak_l: 0,
  peak_r: 0,
  lufs_m: null,
  lufs_s: null,
  recording: false,
  input_peak: null,
  recorded: 0,
}

export default function App() {
  const [view, setView] = useState<View>('projects')
  const [info, setInfo] = useState<AppInfo | null>(null)
  const [project, setProject] = useState<ProjectView | null>(null)
  const [transport, setTransport] = useState<TransportEvent>(IDLE)
  const [worker, setWorker] = useState<WorkerStatus | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const [toasts, setToasts] = useState<{ id: number; text: string }[]>([])
  const toastId = useRef(0)

  const showError = useCallback((text: string) => {
    const id = ++toastId.current
    setToasts((t) => [...t, { id, text }])
    setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), 8000)
  }, [])

  useEffect(() => {
    api.appInfo().then(setInfo).catch((e) => showError(errorMessage(e)))
    api.workerStatus().then(setWorker).catch(() => {})
    const subs = [
      events.transport(setTransport),
      events.workerStatus(setWorker),
      events.pluginFailed((e) => showError(tr('{message}. La aplicación sigue funcionando; el efecto suena sin procesar hasta que lo reinicies.', { message: e.message }))),
      // Una herramienta recién instalada (p. ej. FFmpeg) cambia lo que se puede importar.
      events.worker((ev) => {
        if (ev.event === 'job.done' && ev.data.kind === 'models.install') api.appInfo().then(setInfo).catch(() => {})
      }),
    ]
    return () => {
      subs.forEach((p) => p.then((un) => un()))
    }
  }, [showError])

  const load = useCallback(
    async (label: string, action: () => Promise<ProjectView>) => {
      setBusy(label)
      try {
        const p = await action()
        setProject(p)
        p.plugin_errors.forEach((e) => showError(tr('Plugin no disponible (suena sin procesar): {error}', { error: trBackend(e) })))
        setTransport({ ...IDLE, duration: timelineEnd(p.tracks) })
        setView('studio')
      } catch (e) {
        showError(errorMessage(e))
      } finally {
        setBusy(null)
      }
    },
    [showError],
  )

  const importAudio = useCallback(
    (path: string) => load(tr('Importando y analizando el audio…'), () => api.importAudio(path)),
    [load],
  )
  const openProject = useCallback((dir: string) => load(tr('Abriendo proyecto…'), () => api.openProject(dir)), [load])
  const createProject = useCallback((name: string) => load(tr('Creando proyecto…'), () => api.createProject(name)), [load])

  const recorder = useRecorder(showError)
  const recorderFinished = recorder.finished
  useEffect(() => {
    const un = events.recordFinished((r) => {
      recorderFinished()
      if (r.track) {
        const track = r.track
        const { duration: _d, peaks: _p, ...info } = track
        setProject((p) => p && { ...p, tracks: [...p.tracks, track], project: { ...p.project, tracks: [...p.project.tracks, info] } })
      }
      if (r.message) showError(r.message)
    })
    return () => {
      un.then((f) => f())
    }
  }, [recorderFinished, showError])

  // Arrastrar y soltar archivos sobre la ventana.
  useEffect(() => {
    const un = getCurrentWebview().onDragDropEvent((e) => {
      if (e.payload.type === 'drop' && e.payload.paths.length > 0 && !busy) importAudio(e.payload.paths[0])
    })
    return () => {
      un.then((f) => f())
    }
  }, [busy, importAudio])

  // Ajustes de práctica: actualización optimista; se descartan respuestas antiguas
  // para que arrastrar un control no haga saltar el valor.
  const practiceSeq = useRef(0)
  const projectRef = useRef(project)
  useEffect(() => {
    projectRef.current = project
  }, [project])
  const changePractice = useCallback(
    (patch: Partial<PracticeSettings>) => {
      const current = projectRef.current
      if (!current) return
      const next = { ...current.project.practice, ...patch }
      const withPractice = (p: ProjectView, practice: PracticeSettings) => ({ ...p, project: { ...p.project, practice } })
      projectRef.current = withPractice(current, next)
      setProject((p) => p && withPractice(p, next))
      const seq = ++practiceSeq.current
      api
        .practiceSet(next)
        .then((saved) => {
          if (seq === practiceSeq.current) setProject((p) => p && withPractice(p, saved))
        })
        .catch((e) => showError(errorMessage(e)))
    },
    [showError],
  )

  // Entrenador de velocidad: sube el tempo cada N vueltas del bucle.
  const trainerBase = useRef<number | null>(null)
  const practice = project?.project.practice
  useEffect(() => {
    if (!practice?.trainer_enabled || !practice.loop_enabled) {
      trainerBase.current = null
      return
    }
    if (trainerBase.current === null || transport.loop_count < trainerBase.current) {
      trainerBase.current = transport.loop_count
      return
    }
    const laps = transport.loop_count - trainerBase.current
    if (laps >= practice.trainer_every && practice.tempo < practice.trainer_target) {
      trainerBase.current = transport.loop_count
      changePractice({ tempo: Math.min(practice.trainer_target, +(practice.tempo + practice.trainer_step).toFixed(2)) })
    }
  }, [transport.loop_count, practice, changePractice])

  // Atajos de teclado globales.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement
      if (!project || e.ctrlKey || e.altKey || e.metaKey) return
      if (target.closest('input[type="text"], input[type="number"], textarea, select')) return
      const p = project.project.practice
      const run = (pr: Promise<unknown>) => pr.catch((err) => showError(errorMessage(err)))
      const pos = transport.position
      // Durante la grabación el cabezal no se mueve: solo se puede parar.
      if (transport.recording && !['Space', 'KeyR', 'KeyK'].includes(e.code)) return
      switch (e.code) {
        case 'Space':
          e.preventDefault()
          run(transport.playing || transport.counting_in ? api.pause() : api.play())
          break
        case 'Home':
          run(api.seek(p.loop_enabled ? p.loop_start : 0))
          break
        case 'ArrowLeft':
          e.preventDefault()
          run(api.seek(Math.max(0, pos - (e.shiftKey ? 1 : 5))))
          break
        case 'ArrowRight':
          e.preventDefault()
          run(api.seek(Math.min(transport.duration, pos + (e.shiftKey ? 1 : 5))))
          break
        case 'KeyL':
          if (p.loop_end > p.loop_start) changePractice({ loop_enabled: !p.loop_enabled })
          break
        case 'BracketLeft':
          changePractice({ loop_start: pos, loop_end: p.loop_end > pos ? p.loop_end : Math.min(transport.duration, pos + 8), loop_enabled: true })
          break
        case 'BracketRight':
          changePractice({ loop_start: p.loop_start < pos ? p.loop_start : Math.max(0, pos - 8), loop_end: pos, loop_enabled: true })
          break
        case 'KeyM':
          changePractice({
            markers: [...p.markers, { id: crypto.randomUUID(), name: tr('Marcador {n}', { n: p.markers.length + 1 }), time: pos }],
          })
          break
        case 'Minus':
          changePractice({ tempo: Math.max(0.25, +(p.tempo - 0.05).toFixed(2)) })
          break
        case 'Equal':
          changePractice({ tempo: Math.min(2, +(p.tempo + 0.05).toFixed(2)) })
          break
        case 'Comma':
          changePractice({ pitch: Math.max(-12, p.pitch - 1) })
          break
        case 'Period':
          changePractice({ pitch: Math.min(12, p.pitch + 1) })
          break
        case 'KeyK':
          changePractice({ metronome: !p.metronome })
          break
        case 'KeyR':
          if (transport.recording) recorder.stop()
          else if (recorder.input && !transport.playing) recorder.start(pos)
          break
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [project, transport, showError, changePractice, recorder])

  const onSeparated = useCallback((p: ProjectView) => setProject(p), [])
  const separation = useSeparation(onSeparated, showError)

  const onAnalyzed = useCallback((analysis: Analysis, practice: PracticeSettings) => {
    setProject((p) => p && { ...p, analysis, project: { ...p.project, practice } })
  }, [])
  const analysisJob = useAnalysis(onAnalyzed, showError)
  const onAnalysisChange = useCallback((analysis: Analysis) => {
    setProject((p) => p && { ...p, analysis })
  }, [])

  const onLyrics = useCallback((lyrics: Lyrics) => {
    setProject((p) => p && { ...p, lyrics })
  }, [])
  const lyricsJob = useLyrics(onLyrics, showError)

  const onNotesChange = useCallback((notes: Transcription) => {
    setProject((p) => p && { ...p, notes })
  }, [])
  const transcriptionHandlers = useMemo(
    () => ({
      onNotes: onNotesChange,
      onPitch: (pitch: PitchCurve) => setProject((p) => p && { ...p, pitch }),
      onInstruments: (instruments: InstrumentReport) => setProject((p) => p && { ...p, instruments }),
    }),
    [onNotesChange],
  )
  const transcriptionJobs = useTranscription(transcriptionHandlers, showError)
  const onProjectReplaced = useCallback((p: ProjectView) => setProject(p), [])
  const generationJobs = useGeneration(onProjectReplaced, () => {}, showError)

  const [masterWarnings, setMasterWarnings] = useState<string[]>([])
  const onMastered = useCallback((p: ProjectView, r: MasterReferenceResult) => {
    setProject(p)
    setMasterWarnings(r.warnings.map((w) => trBackend(w)))
  }, [])
  const masterJob = useMasterReference(onMastered, showError)

  const onMasterChange = useCallback((master: MasterSettings) => {
    setProject((p) => p && { ...p, project: { ...p.project, master } })
  }, [])

  const onTrackRemoved = useCallback((id: string) => {
    setProject(
      (p) =>
        p && {
          ...p,
          tracks: p.tracks.filter((t) => t.id !== id),
          project: { ...p.project, tracks: p.project.tracks.filter((t) => t.id !== id) },
        },
    )
  }, [])

  const onClipsChange = useCallback((id: string, clips: Clip[]) => {
    const withClips = <T extends { id: string; clips: Clip[] }>(t: T): T => (t.id === id ? { ...t, clips } : t)
    setProject((p) => p && { ...p, tracks: p.tracks.map(withClips), project: { ...p.project, tracks: p.project.tracks.map(withClips) } })
  }, [])

  const onAutomationChange = useCallback((id: string | null, automation: AutomationLane[]) => {
    setProject((p) => {
      if (!p) return p
      if (id === null) return { ...p, project: { ...p.project, master: { ...p.project.master, automation } } }
      const withLanes = <T extends { id: string; automation: AutomationLane[] }>(t: T): T => (t.id === id ? { ...t, automation } : t)
      return { ...p, tracks: p.tracks.map(withLanes), project: { ...p.project, tracks: p.project.tracks.map(withLanes) } }
    })
  }, [])

  const onTrackChange = useCallback((t: TrackInfo) => {
    setProject((p) => p && { ...p, tracks: p.tracks.map((x) => (x.id === t.id ? { ...x, ...t } : x)) })
  }, [])

  return (
    <div className="app">
      <aside className="sidebar">
        <div className="brand">
          <img src="/logo.svg" alt="" width={28} height={28} />
          <span>MusicNova</span>
        </div>
        <nav>
          {NAV.map((n) => (
            <button
              key={n.id}
              className={`nav-item ${view === n.id ? 'active' : ''}`}
              onClick={() => setView(n.id)}
              disabled={n.id === 'studio' && !project}
            >
              <Icon name={n.icon} />
              <span>{n.label}</span>
            </button>
          ))}
        </nav>
        <div className="sidebar-footer small">
          <span className={`dot ${worker?.state ?? 'stopped'}`} />
          {worker?.state === 'ready' ? tr('IA local: lista') : worker?.state === 'crashed' ? tr('IA local: error') : tr('IA local: iniciando')}
        </div>
        {separation.job && (
          <button className="sidebar-job small" onClick={() => setView('studio')}>
            {tr('Separando… {pct}%', { pct: Math.round(separation.job.progress * 100) })}
            <div className="progress">
              <div style={{ width: `${separation.job.progress * 100}%` }} />
            </div>
          </button>
        )}
        {masterJob.job && (
          <button className="sidebar-job small" onClick={() => setView('studio')}>
            {tr('Masterizando… {pct}%', { pct: Math.round(masterJob.job.progress * 100) })}
            <div className="progress">
              <div style={{ width: `${masterJob.job.progress * 100}%` }} />
            </div>
          </button>
        )}
        {transport.recording && (
          <button className="sidebar-job small rec-status" onClick={() => setView('studio')}>
            ● {tr('Grabando… {s} s', { s: Math.floor(transport.recorded) })}
          </button>
        )}
        {lyricsJob.job && (
          <button className="sidebar-job small" onClick={() => setView('studio')}>
            {tr('Transcribiendo… {pct}%', { pct: Math.round(lyricsJob.job.progress * 100) })}
            <div className="progress">
              <div style={{ width: `${lyricsJob.job.progress * 100}%` }} />
            </div>
          </button>
        )}
        {generationJobs.job && (
          <button className="sidebar-job small" onClick={() => setView('studio')}>
            {GENERATION_LABEL[generationJobs.job.kind]}… {Math.round(generationJobs.job.progress * 100)}%
            <div className="progress">
              <div style={{ width: `${generationJobs.job.progress * 100}%` }} />
            </div>
          </button>
        )}
        {transcriptionJobs.job && (
          <button className="sidebar-job small" onClick={() => setView('studio')}>
            {JOB_LABEL[transcriptionJobs.job.kind]}… {Math.round(transcriptionJobs.job.progress * 100)}%
            <div className="progress">
              <div style={{ width: `${transcriptionJobs.job.progress * 100}%` }} />
            </div>
          </button>
        )}
        {analysisJob.job && (
          <button className="sidebar-job small" onClick={() => setView('studio')}>
            {tr('Analizando… {pct}%', { pct: Math.round(analysisJob.job.progress * 100) })}
            <div className="progress">
              <div style={{ width: `${analysisJob.job.progress * 100}%` }} />
            </div>
          </button>
        )}
      </aside>

      <main className="content">
        {busy && <div className="busy-bar" />}
        {view === 'projects' && (
          <ProjectsView
            info={info}
            busy={busy}
            onImport={importAudio}
            onCreate={createProject}
            onOpen={openProject}
            onError={showError}
          />
        )}
        {view === 'studio' && project && (
          <StudioView
            key={project.dir}
            project={project}
            transport={transport}
            recorder={recorder}
            onTrackRemoved={onTrackRemoved}
            onClipsChange={onClipsChange}
            onAutomationChange={onAutomationChange}
            onTrackChange={onTrackChange}
            onError={showError}
            separation={separation}
            workerReady={worker?.state === 'ready'}
            onOpenModels={() => setView('models')}
            onPracticeChange={changePractice}
            analysisJob={analysisJob}
            onAnalysisChange={onAnalysisChange}
            lyricsJob={lyricsJob}
            onLyricsChange={onLyrics}
            lufs={{ m: transport.lufs_m, s: transport.lufs_s }}
            onMasterChange={onMasterChange}
            masterJob={masterJob}
            masterWarnings={masterWarnings}
            transcriptionJobs={transcriptionJobs}
            generationJobs={generationJobs}
            onNotesChange={onNotesChange}
            onProjectReplaced={onProjectReplaced}
          />
        )}
        {view === 'models' && <ModelsView worker={worker} onError={showError} />}
        {view === 'system' && <SystemView info={info} worker={worker} onError={showError} />}
      </main>

      <TransportBar transport={transport} enabled={!!project} practice={project?.project.practice} onError={showError} />

      <div className="toasts" role="status">
        {toasts.map((t) => (
          <div key={t.id} className="toast">
            <span>{t.text}</span>
            <button className="icon-btn" onClick={() => setToasts((all) => all.filter((x) => x.id !== t.id))}>
              <Icon name="x" size={14} />
            </button>
          </div>
        ))}
      </div>
    </div>
  )
}
