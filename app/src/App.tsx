import { useCallback, useEffect, useRef, useState } from 'react'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import {
  api,
  errorMessage,
  events,
  type Analysis,
  type AppInfo,
  type Lyrics,
  type MasterSettings,
  type PracticeSettings,
  type ProjectView,
  type TrackInfo,
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

type View = 'projects' | 'studio' | 'models' | 'system'

const NAV: { id: View; label: string; icon: string }[] = [
  { id: 'projects', label: 'Proyectos', icon: 'folder' },
  { id: 'studio', label: 'Estudio', icon: 'studio' },
  { id: 'models', label: 'Modelos IA', icon: 'model' },
  { id: 'system', label: 'Sistema', icon: 'system' },
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
        setTransport({ ...IDLE, duration: Math.max(0, ...p.tracks.map((t) => t.duration)) })
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
    (path: string) => load('Importando y analizando el audio…', () => api.importAudio(path)),
    [load],
  )
  const openProject = useCallback((dir: string) => load('Abriendo proyecto…', () => api.openProject(dir)), [load])

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
            markers: [...p.markers, { id: crypto.randomUUID(), name: `Marcador ${p.markers.length + 1}`, time: pos }],
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
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [project, transport, showError, changePractice])

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

  const onMasterChange = useCallback((master: MasterSettings) => {
    setProject((p) => p && { ...p, project: { ...p.project, master } })
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
          IA local: {worker?.state === 'ready' ? 'lista' : worker?.state === 'crashed' ? 'error' : 'iniciando'}
        </div>
        {separation.job && (
          <button className="sidebar-job small" onClick={() => setView('studio')}>
            Separando… {Math.round(separation.job.progress * 100)}%
            <div className="progress">
              <div style={{ width: `${separation.job.progress * 100}%` }} />
            </div>
          </button>
        )}
        {lyricsJob.job && (
          <button className="sidebar-job small" onClick={() => setView('studio')}>
            Transcribiendo… {Math.round(lyricsJob.job.progress * 100)}%
            <div className="progress">
              <div style={{ width: `${lyricsJob.job.progress * 100}%` }} />
            </div>
          </button>
        )}
        {analysisJob.job && (
          <button className="sidebar-job small" onClick={() => setView('studio')}>
            Analizando… {Math.round(analysisJob.job.progress * 100)}%
            <div className="progress">
              <div style={{ width: `${analysisJob.job.progress * 100}%` }} />
            </div>
          </button>
        )}
      </aside>

      <main className="content">
        {busy && <div className="busy-bar" />}
        {view === 'projects' && (
          <ProjectsView info={info} busy={busy} onImport={importAudio} onOpen={openProject} onError={showError} />
        )}
        {view === 'studio' && project && (
          <StudioView
            project={project}
            position={transport.position}
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
