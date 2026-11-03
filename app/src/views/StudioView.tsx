import { Fragment, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { ask } from '@tauri-apps/plugin-dialog'
import {
  api,
  errorMessage,
  timelineEnd,
  type Analysis,
  type AutomationLane,
  type Clip,
  type EffectSlot,
  type Lyrics,
  type MasterSettings,
  type Note,
  type PluginParam,
  type PracticeSettings,
  type ProjectView,
  type SeparationResult,
  type TrackInfo,
  type TrackView,
  type Transcription,
  type TransportEvent,
} from '../lib/api'
import { NotesLane } from '../components/NotesLane'
import { TranscriptionPanel } from '../components/TranscriptionPanel'
import { GenerationPanel } from '../components/GenerationPanel'
import { InstrumentsPanel } from '../components/InstrumentsPanel'
import { useLivePlay } from '../lib/live'
import type { GenerationJobs } from '../lib/useGeneration'
import type { TranscriptionJobs } from '../lib/useTranscription'
import { RecordPanel } from '../components/RecordPanel'
import type { Recorder } from '../lib/useRecorder'
import { PracticePanel } from '../components/PracticePanel'
import type { SeparationJob } from '../lib/useSeparation'
import { SeparationPanel } from '../components/SeparationPanel'
import { ExportPanel } from '../components/ExportPanel'
import { Icon } from '../components/Icon'
import { formatTime } from '../lib/format'
import { TRACK_KINDS } from '../lib/tracks'
import { ClipLane } from '../components/ClipLane'
import { snapGrid, snapTo, splitAt } from '../lib/clips'
import { AutomationLaneView } from '../components/AutomationLaneView'
import { automated, channelTargets, newLane, pruneLanes, retarget, targetKey } from '../lib/automation'
import { AnalysisPanel } from '../components/AnalysisPanel'
import { AnalysisLanes } from '../components/AnalysisLanes'
import type { AnalysisJob } from '../lib/useAnalysis'
import type { LyricsJob } from '../lib/useLyrics'
import { LyricsPanel } from '../components/LyricsPanel'
import { MixPanel } from '../components/MixPanel'
import { MasterPanel } from '../components/MasterPanel'
import type { MasterJob } from '../lib/useMasterReference'
import { usePref } from '../lib/prefs'
import type { Notation } from '../lib/chords'
import { tr, plural } from '../i18n'

interface Props {
  project: ProjectView
  transport: TransportEvent
  recorder: Recorder
  onTrackRemoved: (trackId: string) => void
  onClipsChange: (trackId: string, clips: Clip[]) => void
  /** `trackId = null`: automatización del máster. */
  onAutomationChange: (trackId: string | null, lanes: AutomationLane[]) => void
  onTrackChange: (t: TrackInfo) => void
  onError: (msg: string) => void
  separation: {
    job: SeparationJob | null
    lastResult: SeparationResult | null
    start: (modelId: string, device: string) => void
    cancel: () => void
  }
  workerReady: boolean
  onOpenModels: () => void
  onPracticeChange: (patch: Partial<PracticeSettings>) => void
  analysisJob: {
    job: AnalysisJob | null
    start: (device: string, beatModelId?: string) => void
    cancel: () => void
  }
  onAnalysisChange: (a: Analysis) => void
  lyricsJob: {
    job: LyricsJob | null
    start: (modelId: string | undefined, language: string, device: string) => void
    cancel: () => void
  }
  onLyricsChange: (l: Lyrics) => void
  lufs: { m: number | null; s: number | null }
  onMasterChange: (m: MasterSettings) => void
  masterJob: { job: MasterJob | null; start: (reference: string) => void; cancel: () => void }
  masterWarnings: string[]
  transcriptionJobs: TranscriptionJobs
  generationJobs: GenerationJobs
  onNotesChange: (t: Transcription) => void
  /** El proyecto cambió entero (pistas nuevas generadas en el núcleo). */
  onProjectReplaced: (p: ProjectView) => void
}

type Panel = 'separate' | 'export' | 'practice' | 'analysis' | 'lyrics' | 'mix' | 'master' | 'record' | 'transcription' | 'generate' | 'instruments'

const HEADER_W = 280
const MAX_ZOOM = 64
/** Separaciones posibles entre marcas de la regla (s). */
const TICK_STEPS = [0.1, 0.25, 0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300]

/** Una entrada del historial: cambios de clips o de automatización en uno o varios canales. */
type Change =
  | { kind: 'clips'; trackId: string; before: Clip[]; after: Clip[] }
  | { kind: 'automation'; trackId: string | null; before: AutomationLane[]; after: AutomationLane[] }
  | { kind: 'notes'; trackId: string; before: Note[]; after: Note[] }
type Edit = Change[]
/** Clave de canal para mostrar u ocultar su automatización. */
const MASTER_KEY = 'master'

export function StudioView({
  project,
  transport,
  recorder,
  onTrackRemoved,
  onClipsChange,
  onAutomationChange,
  onTrackChange,
  onError,
  separation,
  workerReady,
  onOpenModels,
  onPracticeChange,
  analysisJob,
  onAnalysisChange,
  lyricsJob,
  onLyricsChange,
  lufs,
  onMasterChange,
  masterJob,
  masterWarnings,
  transcriptionJobs,
  generationJobs,
  onNotesChange,
  onProjectReplaced,
}: Props) {
  const position = transport.position
  const recording = transport.recording
  const practice = project.project.practice
  const hasStems = project.tracks.some((t) => t.kind !== 'mix')
  const hasMix = project.tracks.some((t) => t.kind === 'mix')
  const analysis = project.analysis
  const [panel, setPanel] = useState<Panel | null>(
    !hasMix ? 'record' : hasStems ? (analysis ? 'analysis' : 'practice') : 'separate',
  )
  const shownPanel = recording
    ? 'record'
    : separation.job
    ? 'separate'
    : analysisJob.job
      ? 'analysis'
      : lyricsJob.job
        ? 'lyrics'
        : masterJob.job
          ? 'master'
          : transcriptionJobs.job
            ? 'transcription'
            : generationJobs.job
              ? 'generate'
              : panel
  const [notation, setNotation] = usePref<Notation>('notation', 'english', ['english', 'latin'])
  const [instrument, setInstrument] = usePref<'guitar' | 'piano'>('instrument', 'guitar', ['guitar', 'piano'])

  const hasVocalsStem = project.tracks.some((t) => t.file.startsWith('audio/stems/') && /\/vocals\.\w+$/.test(t.file))
  const [mixChannel, setMixChannel] = useState<string | null>(null)
  /** Parámetros de los plugins del proyecto (por id de efecto), para automatizarlos. */
  const [pluginParams, setPluginParams] = useState<Record<string, PluginParam[]>>({})
  const pluginSlotIds = [...project.tracks.flatMap((t) => t.effects), ...project.project.master.effects]
    .filter((e) => e.params.type === 'plugin')
    .map((e) => e.id)
    .join(',')
  useEffect(() => {
    let alive = true
    const ids = pluginSlotIds ? pluginSlotIds.split(',') : []
    Promise.all(ids.map((id) => api.pluginParams(id).then((p) => [id, p] as const).catch(() => [id, [] as PluginParam[]] as const))).then(
      (list) => alive && setPluginParams(Object.fromEntries(list)),
    )
    return () => {
      alive = false
    }
  }, [pluginSlotIds])
  const master = project.project.master
  const setTrackEffects = (trackId: string, effects: EffectSlot[]) => {
    const t = project.tracks.find((x) => x.id === trackId)
    if (!t) return
    onTrackChange({ ...t, effects })
    // Al quitar un efecto desaparecen sus curvas (el núcleo hace lo mismo).
    const lanes = pruneLanes(t.automation, effects)
    if (lanes.length !== t.automation.length) onAutomationChange(trackId, lanes)
    api.effectsSet(trackId, effects).catch((err) => onError(errorMessage(err)))
  }
  const setMasterEffects = (effects: EffectSlot[]) => {
    onMasterChange({ ...master, effects, automation: pruneLanes(master.automation, effects) })
    api.effectsSet(null, effects).catch((err) => onError(errorMessage(err)))
  }
  const setMasterGain = (gain_db: number) => {
    onMasterChange({ ...master, gain_db })
    api.masterGainSet(gain_db).catch((err) => onError(errorMessage(err)))
  }
  // A/B del master por referencia: A silencia la pista de master; B la deja sola.
  const toggleMasterAB = (t: TrackView) => update(t, t.solo ? { solo: false, mute: true } : { solo: true, mute: false })

  const openFx = (trackId: string) => {
    setMixChannel(trackId)
    setPanel('mix')
  }

  const saveLyrics = (l: Lyrics) => {
    onLyricsChange(l)
    api.lyricsUpdate(l).then(onLyricsChange).catch((err) => onError(errorMessage(err)))
  }

  const saveAnalysis = (a: Analysis) => {
    onAnalysisChange(a)
    api.analysisUpdate(a).then(onAnalysisChange).catch((err) => onError(errorMessage(err)))
  }
  const tracksEnd = useMemo(() => timelineEnd(project.tracks), [project])
  // Mientras se arrastra un clip la escala no cambia (si no, el clip «huiría» del ratón).
  const [frozenDuration, setFrozenDuration] = useState<number | null>(null)
  // Mientras se graba, la línea de tiempo crece con la toma.
  const duration = frozenDuration ?? Math.max(0.001, tracksEnd, recording ? position + 1 : 0)
  const anySolo = project.tracks.some((t) => t.solo)
  const lanesRef = useRef<HTMLDivElement>(null)
  const scrollRef = useRef<HTMLDivElement>(null)

  // ---- Zoom horizontal ----------------------------------------------------
  const [zoom, setZoomState] = useState(1)
  const [viewW, setViewW] = useState(1000)
  const zoomAnchor = useRef<{ time: number; clientX: number } | null>(null)
  useEffect(() => {
    const el = scrollRef.current
    if (!el) return
    const ro = new ResizeObserver(() => setViewW(el.clientWidth))
    ro.observe(el)
    return () => ro.disconnect()
  }, [])
  const laneW = Math.max(1, (viewW - HEADER_W) * zoom)
  const setZoom = useCallback(
    (z: number, anchorClientX?: number) => {
      const el = scrollRef.current
      const next = Math.min(MAX_ZOOM, Math.max(1, z))
      if (el) {
        const rect = el.getBoundingClientRect()
        const clientX = anchorClientX ?? rect.left + HEADER_W + (rect.width - HEADER_W) / 2
        const x = el.scrollLeft + clientX - rect.left - HEADER_W
        zoomAnchor.current = { time: Math.max(0, x / laneW) * duration, clientX }
      }
      setZoomState(next)
    },
    [laneW, duration],
  )
  // Tras cambiar el zoom, el instante bajo el ratón (o el centro) se queda en su sitio.
  useLayoutEffect(() => {
    const el = scrollRef.current
    const a = zoomAnchor.current
    if (!el || !a) return
    zoomAnchor.current = null
    const rect = el.getBoundingClientRect()
    el.scrollLeft = (a.time / duration) * laneW - (a.clientX - rect.left - HEADER_W)
  }, [zoom, laneW, duration])
  // Ctrl + rueda: zoom (el listener no puede ser pasivo para evitar el desplazamiento).
  useEffect(() => {
    const el = scrollRef.current
    if (!el) return
    const onWheel = (e: WheelEvent) => {
      if (!e.ctrlKey) return
      e.preventDefault()
      setZoom(zoom * (e.deltaY < 0 ? 1.25 : 0.8), e.clientX)
    }
    el.addEventListener('wheel', onWheel, { passive: false })
    return () => el.removeEventListener('wheel', onWheel)
  }, [zoom, setZoom])
  // Con zoom, la vista sigue al cabezal mientras suena.
  useEffect(() => {
    const el = scrollRef.current
    if (!el || zoom <= 1 || !transport.playing) return
    const x = (position / duration) * laneW
    const visible = el.clientWidth - HEADER_W
    if (x < el.scrollLeft || x > el.scrollLeft + visible - 40) el.scrollLeft = x - 40
  }, [position, zoom, laneW, duration, transport.playing])

  // ---- Edición de clips -----------------------------------------------------
  const [selected, setSelected] = useState<{ trackId: string; clipId: string } | null>(null)
  const [snapOn, setSnapOn] = usePref<'on' | 'off'>('snap', 'on', ['on', 'off'])
  const grid = useMemo(() => snapGrid(practice, analysis, tracksEnd), [practice, analysis, tracksEnd])
  const snap = useCallback(
    (t: number, pxPerSec: number, free: boolean) => (snapOn === 'off' || free ? t : snapTo(grid, t, 10 / pxPerSec)),
    [snapOn, grid],
  )
  const undoStack = useRef<Edit[]>([])
  const redoStack = useRef<Edit[]>([])
  // Tamaños de las pilas para la interfaz (las pilas viven en refs). El
  // historial se reinicia al cambiar de proyecto porque la vista se monta de nuevo.
  const [history, setHistory] = useState({ undo: 0, redo: 0 })
  const syncHistory = () => setHistory({ undo: undoStack.current.length, redo: redoStack.current.length })

  const saveClips = useCallback(
    (trackId: string, clips: Clip[]) => {
      onClipsChange(trackId, clips)
      api
        .clipsSet(trackId, clips)
        .then((saved) => onClipsChange(trackId, saved))
        .catch((err) => onError(errorMessage(err)))
    },
    [onClipsChange, onError],
  )
  const saveAutomation = useCallback(
    (trackId: string | null, lanes: AutomationLane[]) => {
      onAutomationChange(trackId, lanes)
      api
        .automationSet(trackId, lanes)
        .then((saved) => onAutomationChange(trackId, saved))
        .catch((err) => onError(errorMessage(err)))
    },
    [onAutomationChange, onError],
  )
  // ---- Notas (piano roll) ------------------------------------------------------
  const [shownNotes, setShownNotes] = useState<Set<string>>(new Set())
  const toggleNotes = (trackId: string) =>
    setShownNotes((s) => {
      const n = new Set(s)
      if (n.has(trackId)) n.delete(trackId)
      else n.add(trackId)
      return n
    })
  // Las notas se transcriben y se muestran a la vez.
  const prevNotes = useRef(project.notes)
  useEffect(() => {
    const before = new Set(prevNotes.current?.tracks.map((t) => t.track_id))
    prevNotes.current = project.notes
    const fresh = project.notes?.tracks.filter((t) => !before.has(t.track_id)).map((t) => t.track_id) ?? []
    if (fresh.length) setShownNotes((s) => new Set([...s, ...fresh]))
  }, [project.notes])
  const notesRef = useRef(project.notes)
  useEffect(() => {
    notesRef.current = project.notes
  })
  /** Sustituye localmente las notas de una pista. */
  const setTrackNotes = useCallback(
    (trackId: string, notes: Note[]) => {
      const cur = notesRef.current ?? { version: 1, tracks: [] }
      const exists = cur.tracks.some((t) => t.track_id === trackId)
      const tracks = exists
        ? cur.tracks.map((t) => (t.track_id === trackId ? { ...t, notes } : t))
        : [...cur.tracks, { track_id: trackId, mode: 'poly' as const, notes }]
      onNotesChange({ ...cur, tracks })
    },
    [onNotesChange],
  )
  // Pistas de instrumento: su audio se regenera al cambiar las notas.
  const projectRef = useRef(project)
  useEffect(() => {
    projectRef.current = project
  })
  const replaceTrack = useCallback(
    (tv: TrackView) => {
      const p = projectRef.current
      onProjectReplaced({
        ...p,
        tracks: p.tracks.map((t) => (t.id === tv.id ? tv : t)),
        project: { ...p.project, tracks: p.project.tracks.map((t) => (t.id === tv.id ? tv : t)) },
      })
    },
    [onProjectReplaced],
  )
  const saveNotes = useCallback(
    (trackId: string, notes: Note[]) => {
      setTrackNotes(trackId, notes)
      const instrument = projectRef.current.tracks.find((t) => t.id === trackId)?.kind === 'instrument'
      api
        .notesUpdate(trackId, notes)
        .then((t) => {
          onNotesChange(t)
          return instrument ? api.instrumentRender(trackId).then(replaceTrack) : undefined
        })
        .catch((err) => onError(errorMessage(err)))
    },
    [setTrackNotes, onNotesChange, onError, replaceTrack],
  )

  /** Aplica un lado (antes o después) de un cambio del historial. */
  const applyChange = useCallback(
    (c: Change, side: 'before' | 'after') => {
      if (c.kind === 'clips') saveClips(c.trackId, c[side])
      else if (c.kind === 'notes') saveNotes(c.trackId, c[side])
      else saveAutomation(c.trackId, c[side])
    },
    [saveClips, saveAutomation, saveNotes],
  )
  const commitEdit = useCallback(
    (edit: Edit) => {
      const changed = edit.filter((e) => JSON.stringify(e.before) !== JSON.stringify(e.after))
      if (changed.length === 0) return
      undoStack.current.push(changed)
      if (undoStack.current.length > 200) undoStack.current.shift()
      redoStack.current = []
      syncHistory()
      changed.forEach((e) => applyChange(e, 'after'))
    },
    [applyChange],
  )

  /** Tocar en vivo: lo grabado se añade a las notas de la pista (con deshacer). */
  const live = useLivePlay({
    playing: transport.playing,
    projectKey: project.dir,
    onRecorded: (trackId, recorded) => {
      const before = projectRef.current.notes?.tracks.find((n) => n.track_id === trackId)?.notes ?? []
      const after = [...before, ...recorded].sort((a, b) => a.start - b.start || a.pitch - b.pitch)
      commitEdit([{ kind: 'notes', trackId, before, after }])
      if (!shownNotes.has(trackId)) toggleNotes(trackId)
    },
    onEditorClosed: (trackId) => {
      if (projectRef.current.tracks.some((t) => t.id === trackId)) api.instrumentRender(trackId).then(replaceTrack).catch((e) => onError(errorMessage(e)))
    },
    onError,
  })

  const undo = useCallback(() => {
    const e = undoStack.current.pop()
    if (!e) return
    redoStack.current.push(e)
    syncHistory()
    e.forEach((c) => applyChange(c, 'before'))
  }, [applyChange])
  const redo = useCallback(() => {
    const e = redoStack.current.pop()
    if (!e) return
    undoStack.current.push(e)
    syncHistory()
    e.forEach((c) => applyChange(c, 'after'))
  }, [applyChange])

  // ---- Automatización ---------------------------------------------------------
  const [expanded, setExpanded] = useState<Set<string>>(new Set())
  const toggleExpanded = (key: string) =>
    setExpanded((s) => {
      const n = new Set(s)
      if (n.has(key)) n.delete(key)
      else n.add(key)
      return n
    })
  /** Cambia los carriles de un canal y lo apunta en el historial. */
  const commitLanes = (trackId: string | null, before: AutomationLane[], after: AutomationLane[]) =>
    commitEdit([{ kind: 'automation', trackId, before, after }])

  /** Carriles de automatización de un canal (pista o máster). */
  const automationRows = (trackId: string | null, lanes: AutomationLane[], effects: EffectSlot[], color: string) => {
    const targets = channelTargets(effects, trackId === null, pluginParams, lanes)
    const used = new Set(lanes.map((l) => targetKey(l.target)))
    return (
      <>
        {lanes.map((lane) => {
          const info = targets.find((t) => t.key === targetKey(lane.target))
          if (!info) return null
          const replace = (next: AutomationLane) => lanes.map((l) => (l.id === lane.id ? next : l))
          return (
            <div key={lane.id} className="lane auto-lane">
              <div className="lane-header auto-header">
                <span className="lane-chip" style={{ background: color, opacity: 0.5 }} />
                <select
                  className="compact"
                  value={info.key}
                  disabled={recording}
                  onChange={(e) => {
                    const to = targets.find((t) => t.key === e.target.value)
                    if (to) commitLanes(trackId, lanes, replace(retarget(lane, info, to)))
                  }}
                >
                  {targets.map((t) => (
                    <option key={t.key} value={t.key} disabled={t.key !== info.key && used.has(t.key)}>
                      {t.label}
                    </option>
                  ))}
                </select>
                <div className="lane-buttons">
                  <label className="check small" title={tr('Activar o desactivar esta curva')}>
                    <input
                      type="checkbox"
                      checked={lane.enabled}
                      disabled={recording}
                      onChange={(e) => commitLanes(trackId, lanes, replace({ ...lane, enabled: e.target.checked }))}
                    />
                  </label>
                  <button
                    className="icon-btn small"
                    title={tr('Borrar todos los puntos')}
                    disabled={recording || lane.points.length === 0}
                    onClick={() => commitLanes(trackId, lanes, replace({ ...lane, points: [] }))}
                  >
                    <Icon name="refresh" size={13} />
                  </button>
                  <button
                    className="icon-btn small"
                    title={tr('Quitar el carril')}
                    disabled={recording}
                    onClick={() => commitLanes(trackId, lanes, lanes.filter((l) => l.id !== lane.id))}
                  >
                    <Icon name="trash" size={13} />
                  </button>
                </div>
              </div>
              <AutomationLaneView
                lane={lane}
                info={info}
                duration={duration}
                color={color}
                locked={recording}
                snap={snap}
                onPreview={(next) => onAutomationChange(trackId, replace(next))}
                onCommit={(before, after) =>
                  commitLanes(
                    trackId,
                    lanes.map((l) => (l.id === lane.id ? before : l)),
                    lanes.map((l) => (l.id === lane.id ? after : l)),
                  )
                }
              />
            </div>
          )
        })}
        <div className="lane auto-lane auto-add">
          <div className="lane-header auto-header">
            <span className="lane-chip" style={{ background: color, opacity: 0.5 }} />
            <button
              className="link small"
              disabled={recording || used.size >= targets.length}
              onClick={() => {
                const lane = newLane(lanes, targets)
                if (lane) commitLanes(trackId, lanes, [...lanes, lane])
              }}
            >
              {tr('+ Carril de automatización')}
            </button>
          </div>
          <div className="auto-hint small muted">
            Clic: añadir punto · arrastrar: mover (Mayús: solo el valor) · doble clic o clic derecho: borrar
          </div>
        </div>
      </>
    )
  }

  /** Divide en el cabezal el clip seleccionado o, sin selección, todos los clips bajo el cabezal. */
  const splitAtPlayhead = useCallback(() => {
    const edit: Edit = []
    for (const t of project.tracks) {
      if (selected && selected.trackId !== t.id) continue
      const after = splitAt(t.clips, position, selected?.clipId)
      if (after) edit.push({ kind: 'clips', trackId: t.id, before: t.clips, after })
    }
    commitEdit(edit)
  }, [project.tracks, selected, position, commitEdit])

  const deleteSelected = useCallback(() => {
    if (!selected) return
    const t = project.tracks.find((x) => x.id === selected.trackId)
    if (!t) return
    commitEdit([{ kind: 'clips', trackId: t.id, before: t.clips, after: t.clips.filter((c) => c.id !== selected.clipId) }])
    setSelected(null)
  }, [project.tracks, selected, commitEdit, setSelected])

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement
      if (target.closest('input[type="text"], input[type="number"], textarea, select') || recording) return
      const mod = e.ctrlKey || e.metaKey
      if (mod && e.code === 'KeyZ') {
        e.preventDefault()
        if (e.shiftKey) redo()
        else undo()
      } else if (mod && e.code === 'KeyY') {
        e.preventDefault()
        redo()
      } else if (!mod && !e.altKey && e.code === 'KeyS') {
        splitAtPlayhead()
      } else if (!mod && (e.code === 'Delete' || e.code === 'Backspace')) {
        e.preventDefault()
        deleteSelected()
      } else if (!mod && e.code === 'Escape') {
        setSelected(null)
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [undo, redo, splitAtPlayhead, deleteSelected, recording])

  const timeAt = (clientX: number): number | null => {
    const el = lanesRef.current
    if (!el) return null
    const rect = el.getBoundingClientRect()
    const x = clientX - rect.left - HEADER_W
    if (x < 0) return null
    return Math.max(0, Math.min(1, x / (rect.width - HEADER_W))) * duration
  }

  const seek = (t: number) => {
    if (!recording) api.seek(t).catch((err) => onError(errorMessage(err)))
  }

  const trackNotes = (id: string) => project.notes?.tracks.find((t) => t.track_id === id)
  const pitchFor = (id: string) => (project.pitch?.track_id === id ? project.pitch : null)
  /** Tomas y pistas generadas (síntesis MIDI, voz afinada, sin ruido, acompañamientos de IA). */
  const removable = (t: TrackView) =>
    t.kind === 'recording' ||
    t.kind === 'instrument' ||
    (t.kind === 'generated' && ['audio/midi/', 'audio/tuned/', 'audio/denoised/', 'audio/generated/'].some((d) => t.file.startsWith(d)))
  const removeTrack = async (t: TrackView) => {
    const ok = await ask(tr('¿Eliminar «{name}»? Su audio se borrará del proyecto.', { name: t.name }), {
      title: tr('Eliminar pista'),
      kind: 'warning',
      okLabel: tr('Eliminar'),
      cancelLabel: tr('Cancelar'),
    })
    if (!ok) return
    api
      .trackRemove(t.id)
      .then(() => onTrackRemoved(t.id))
      .catch((err) => onError(errorMessage(err)))
  }

  // Arrastrar sobre la regla crea un bucle; un clic simple solo salta.
  const [dragLoop, setDragLoop] = useState<{ from: number; to: number } | null>(null)
  const onRulerDown = (e: React.MouseEvent) => {
    const from = timeAt(e.clientX)
    if (from == null) return
    let to = from
    const move = (ev: MouseEvent) => {
      to = timeAt(ev.clientX) ?? to
      setDragLoop({ from, to })
    }
    const up = () => {
      window.removeEventListener('mousemove', move)
      window.removeEventListener('mouseup', up)
      setDragLoop(null)
      const [a, b] = from < to ? [from, to] : [to, from]
      if (b - a > 0.25) {
        onPracticeChange({ loop_start: a, loop_end: b, loop_enabled: true })
        seek(a)
      } else {
        seek(from)
      }
    }
    window.addEventListener('mousemove', move)
    window.addEventListener('mouseup', up)
  }

  const loopView = dragLoop
    ? { a: Math.min(dragLoop.from, dragLoop.to), b: Math.max(dragLoop.from, dragLoop.to), on: true }
    : practice.loop_end > practice.loop_start
      ? { a: practice.loop_start, b: practice.loop_end, on: practice.loop_enabled }
      : null

  const update = (t: TrackView, patch: Partial<TrackInfo>) => {
    onTrackChange({ ...t, ...patch })
    api.updateTrack(t.id, patch).then(onTrackChange).catch((err) => onError(errorMessage(err)))
  }

  const tickStep = useMemo(() => {
    const pxPerSec = laneW / duration
    return TICK_STEPS.find((st) => st * pxPerSec >= 70) ?? TICK_STEPS[TICK_STEPS.length - 1]
  }, [laneW, duration])
  const ticks = useMemo(() => {
    const out: number[] = []
    for (let i = 0; i * tickStep <= duration && out.length < 5000; i++) out.push(+(i * tickStep).toFixed(3))
    return out
  }, [duration, tickStep])
  const selectedTrack = selected ? project.tracks.find((t) => t.id === selected.trackId) : undefined

  const playheadPct = `${Math.min(100, (position / duration) * 100)}%`

  return (
    <section className="studio">
      <header className="view-header">
        <div>
          <h1>{project.project.name}</h1>
          <p className="muted">
            {plural(project.tracks.length, '{n} pista', '{n} pistas')} · {formatTime(tracksEnd)} ·{' '}
            {project.sample_rate / 1000} kHz
          </p>
        </div>
        <div className="toolbar">
          <button
            className={`btn ${shownPanel === 'record' ? 'active' : ''}`}
            onClick={() => setPanel(panel === 'record' ? null : 'record')}
          >
            <Icon name="mic" size={16} /> {tr('Grabar')}
          </button>
          <button
            className={`btn ${shownPanel === 'separate' ? 'active' : ''}`}
            onClick={() => setPanel(panel === 'separate' ? null : 'separate')}
          >
            <Icon name="model" size={16} /> {hasStems ? tr('Volver a separar') : tr('Separar stems')}
          </button>
          <button
            className={`btn ${shownPanel === 'analysis' ? 'active' : ''}`}
            onClick={() => setPanel(panel === 'analysis' ? null : 'analysis')}
          >
            <Icon name="chord" size={16} /> {tr('Análisis')}
          </button>
          <button
            className={`btn ${shownPanel === 'transcription' ? 'active' : ''}`}
            onClick={() => setPanel(panel === 'transcription' ? null : 'transcription')}
          >
            <Icon name="piano" size={16} /> {tr('Transcripción')}
          </button>
          <button
            className={`btn ${shownPanel === 'instruments' ? 'active' : ''}`}
            onClick={() => setPanel(panel === 'instruments' ? null : 'instruments')}
            title={tr('Pistas de instrumento: notas MIDI con sintetizador o plugin')}
          >
            <Icon name="music" size={16} /> {tr('Instrumentos')}
          </button>
          <button
            className={`btn ${shownPanel === 'generate' ? 'active' : ''}`}
            onClick={() => setPanel(panel === 'generate' ? null : 'generate')}
            title={tr('Limpiar ruido y generar acompañamientos con IA')}
          >
            <Icon name="sparkle" size={16} /> {tr('IA generativa')}
          </button>
          <button
            className={`btn ${shownPanel === 'lyrics' ? 'active' : ''}`}
            onClick={() => setPanel(panel === 'lyrics' ? null : 'lyrics')}
          >
            <Icon name="lyrics" size={16} /> {tr('Letra')}
          </button>
          <button
            className={`btn ${shownPanel === 'mix' ? 'active' : ''}`}
            onClick={() => setPanel(panel === 'mix' ? null : 'mix')}
          >
            <Icon name="sliders" size={16} /> {tr('Mezcla')}
          </button>
          <button
            className={`btn ${shownPanel === 'master' ? 'active' : ''}`}
            onClick={() => setPanel(panel === 'master' ? null : 'master')}
          >
            <Icon name="star" size={16} /> {tr('Máster')}
          </button>
          <button
            className={`btn ${shownPanel === 'practice' ? 'active' : ''}`}
            onClick={() => setPanel(panel === 'practice' ? null : 'practice')}
          >
            <Icon name="music" size={16} /> {tr('Práctica')}
          </button>
          <button
            className={`btn ${shownPanel === 'export' ? 'active' : ''}`}
            onClick={() => setPanel(panel === 'export' ? null : 'export')}
          >
            <Icon name="download" size={16} /> {tr('Exportar')}
          </button>
        </div>
      </header>

      {shownPanel === 'record' && (
        <RecordPanel recorder={recorder} transport={transport} practice={practice} onPracticeChange={onPracticeChange} />
      )}
      {shownPanel === 'separate' && (
        <SeparationPanel
          job={separation.job}
          lastResult={separation.lastResult}
          workerReady={workerReady}
          onStart={separation.start}
          onCancel={separation.cancel}
          onOpenModels={onOpenModels}
          onError={onError}
        />
      )}
      {shownPanel === 'export' && <ExportPanel project={project} onError={onError} />}
      {shownPanel === 'master' && (
        <MasterPanel
          tracks={project.tracks}
          workerReady={workerReady}
          job={masterJob.job}
          lastWarnings={masterWarnings}
          onStartReference={masterJob.start}
          onCancelReference={masterJob.cancel}
          onMasterChange={onMasterChange}
          onToggleSolo={toggleMasterAB}
          onOpenMix={() => {
            setMixChannel(null)
            setPanel('mix')
          }}
          onError={onError}
        />
      )}
      {shownPanel === 'mix' && (
        <MixPanel
          tracks={project.tracks}
          master={master}
          selected={mixChannel}
          lufs={lufs}
          onSelect={setMixChannel}
          onTrackEffects={setTrackEffects}
          onMasterEffects={setMasterEffects}
          onMasterGain={setMasterGain}
        />
      )}
      {shownPanel === 'lyrics' && (
        <LyricsPanel
          lyrics={project.lyrics}
          projectName={project.project.name}
          hasVocalsStem={hasVocalsStem}
          position={position}
          job={lyricsJob.job}
          workerReady={workerReady}
          onStart={lyricsJob.start}
          onCancel={lyricsJob.cancel}
          onChange={saveLyrics}
          onSeek={seek}
          onOpenModels={onOpenModels}
          onError={onError}
        />
      )}
      {shownPanel === 'instruments' && (
        <InstrumentsPanel
          live={live}
          project={project}
          shownNotes={shownNotes}
          onToggleNotes={toggleNotes}
          onProjectReplaced={onProjectReplaced}
          onTrackReplaced={replaceTrack}
          onError={onError}
        />
      )}
      {shownPanel === 'generate' && (
        <GenerationPanel
          project={project}
          jobs={generationJobs}
          workerReady={workerReady}
          notation={notation}
          onProjectReplaced={onProjectReplaced}
          onOpenModels={onOpenModels}
          onError={onError}
        />
      )}
      {shownPanel === 'transcription' && (
        <TranscriptionPanel
          project={project}
          jobs={transcriptionJobs}
          workerReady={workerReady}
          notation={notation}
          shownNotes={shownNotes}
          onToggleNotes={toggleNotes}
          onNotesChange={onNotesChange}
          onProjectReplaced={onProjectReplaced}
          onSeek={seek}
          onOpenModels={onOpenModels}
          onError={onError}
        />
      )}
      {shownPanel === 'analysis' && (
        <AnalysisPanel
          analysis={analysis}
          practice={practice}
          position={position}
          job={analysisJob.job}
          workerReady={workerReady}
          notation={notation}
          instrument={instrument}
          onNotation={setNotation}
          onInstrument={setInstrument}
          onStart={analysisJob.start}
          onCancel={analysisJob.cancel}
          onPracticeChange={onPracticeChange}
          onOpenModels={onOpenModels}
          onError={onError}
        />
      )}
      {shownPanel === 'practice' && (
        <PracticePanel
          practice={practice}
          position={position}
          duration={duration}
          onChange={onPracticeChange}
          onSeek={seek}
        />
      )}

      <div className="edit-bar">
        <button className="btn ghost small" onClick={undo} disabled={history.undo === 0 || recording} title={tr('Deshacer (Ctrl+Z)')}>
          <Icon name="undo" size={15} /> {tr('Deshacer')}
        </button>
        <button className="btn ghost small" onClick={redo} disabled={history.redo === 0 || recording} title={tr('Rehacer (Ctrl+Mayús+Z)')}>
          <Icon name="redo" size={15} /> {tr('Rehacer')}
        </button>
        <button
          className="btn ghost small"
          onClick={splitAtPlayhead}
          disabled={recording}
          title={tr('Dividir en el cabezal el clip seleccionado o, sin selección, todos los clips bajo el cabezal (S)')}
        >
          <Icon name="scissors" size={15} /> {tr('Dividir')}
        </button>
        <button className="btn ghost small" onClick={deleteSelected} disabled={!selectedTrack || recording} title={tr('Eliminar el clip seleccionado (Supr)')}>
          <Icon name="trash" size={15} /> {tr('Eliminar clip')}
        </button>
        <label className="check small" title={tr('Al mover o recortar, los bordes se pegan a los pulsos (mantén Alt para moverlos libremente)')}>
          <input type="checkbox" checked={snapOn === 'on'} onChange={(e) => setSnapOn(e.target.checked ? 'on' : 'off')} />
          {tr('Ajustar a pulsos')}
        </label>
        <div className="zoom">
          <button className="icon-btn small" onClick={() => setZoom(zoom / 1.5)} disabled={zoom <= 1} title={tr('Alejar')}>
            −
          </button>
          <input
            type="range"
            min={0}
            max={Math.log2(MAX_ZOOM)}
            step={0.05}
            value={Math.log2(zoom)}
            onChange={(e) => setZoom(2 ** Number(e.target.value))}
            aria-label={tr('Zoom')}
            title={tr('Zoom (Ctrl + rueda)')}
          />
          <button className="icon-btn small" onClick={() => setZoom(zoom * 1.5)} disabled={zoom >= MAX_ZOOM} title={tr('Acercar')}>
            +
          </button>
          <span className="small mono muted">{zoom < 10 ? zoom.toFixed(1) : Math.round(zoom)}×</span>
        </div>
      </div>

      <div className="lanes-scroll" ref={scrollRef}>
      <div
        className="lanes"
        ref={lanesRef}
        style={{ ['--header-w' as string]: `${HEADER_W}px`, width: `${HEADER_W + laneW}px`, minWidth: '100%' }}
      >
        <div className="ruler" onMouseDown={onRulerDown}>
          <div className="ruler-spacer" />
          <div className="ruler-track">
            {ticks.map((s) => (
              <span key={s} className="tick" style={{ left: `${(s / duration) * 100}%` }}>
                {tickStep < 1 ? formatTime(s) : formatTime(s).slice(0, -3)}
              </span>
            ))}
          </div>
        </div>

        {analysis && (
          <AnalysisLanes
            analysis={analysis}
            practice={practice}
            duration={duration}
            position={position}
            notation={notation}
            onSeek={seek}
            onLoop={(a, b) => {
              onPracticeChange({ loop_start: a, loop_end: b, loop_enabled: true })
              seek(a)
            }}
            onChange={saveAnalysis}
            onError={onError}
          />
        )}

        {project.tracks.map((t) => {
          const kind = TRACK_KINDS[t.kind]
          const audible = !t.mute && (!anySolo || t.solo)
          const volAuto = automated(t.automation, 'volume')
          const panAuto = automated(t.automation, 'pan')
          return (
            <Fragment key={t.id}>
            <div className="lane">
              <div className="lane-header">
                <span className="lane-chip" style={{ background: kind.color }} />
                <div className="lane-title">
                  <strong title={t.name}>{t.name}</strong>
                  <span className="muted small">{kind.label}</span>
                </div>
                <div className="lane-buttons">
                  <button
                    className={`toggle mute ${t.mute ? 'on' : ''}`}
                    title={tr('Silenciar (M)')}
                    onClick={() => update(t, { mute: !t.mute })}
                  >
                    M
                  </button>
                  <button
                    className={`toggle solo ${t.solo ? 'on' : ''}`}
                    title={tr('Solo (S)')}
                    onClick={() => update(t, { solo: !t.solo })}
                  >
                    S
                  </button>
                  <button
                    className={`toggle fx ${t.effects.some((e) => e.enabled) ? 'on' : ''}`}
                    title={t.effects.length ? tr('Efectos: {n}', { n: t.effects.length }) : tr('Añadir efectos')}
                    onClick={() => openFx(t.id)}
                  >
                    FX
                  </button>
                  <button
                    className={`toggle auto ${expanded.has(t.id) ? 'open' : ''} ${t.automation.some((l) => l.enabled && l.points.length) ? 'on' : ''}`}
                    title={t.automation.length ? plural(t.automation.length, 'Automatización: {n} curva', 'Automatización: {n} curvas') : tr('Automatización')}
                    onClick={() => toggleExpanded(t.id)}
                  >
                    A
                  </button>
                  {(trackNotes(t.id) || pitchFor(t.id)) && (
                    <button
                      className={`toggle notes ${shownNotes.has(t.id) ? 'open on' : ''}`}
                      title={shownNotes.has(t.id) ? tr('Ocultar el piano roll') : tr('Ver el piano roll (notas transcritas y curva de tono)')}
                      onClick={() => toggleNotes(t.id)}
                    >
                      N
                    </button>
                  )}
                  {removable(t) && (
                    <button
                      className="icon-btn lane-remove"
                      title={tr('Eliminar la pista')}
                      disabled={recording}
                      onClick={() => removeTrack(t)}
                    >
                      <Icon name="trash" size={14} />
                    </button>
                  )}
                </div>
                <label
                  className={`lane-slider ${volAuto ? 'automated' : ''}`}
                  title={volAuto ? tr('Volumen controlado por la automatización') : tr('Volumen: {db} dB (doble clic: 0 dB)', { db: t.gain_db.toFixed(1) })}
                >
                  <span className="small muted">{tr('Vol')}</span>
                  <input
                    disabled={volAuto}
                    type="range"
                    min={-60}
                    max={12}
                    step={0.5}
                    value={t.gain_db}
                    onChange={(e) => update(t, { gain_db: Number(e.target.value) })}
                    onDoubleClick={() => update(t, { gain_db: 0 })}
                  />
                  <span className="small mono">{t.gain_db <= -60 ? '-∞' : t.gain_db.toFixed(1)}</span>
                </label>
                <label
                  className={`lane-slider ${panAuto ? 'automated' : ''}`}
                  title={panAuto ? tr('Panorama controlado por la automatización') : tr('Panorama (doble clic: centro)')}
                >
                  <span className="small muted">{tr('Pan')}</span>
                  <input
                    disabled={panAuto}
                    type="range"
                    min={-1}
                    max={1}
                    step={0.02}
                    value={t.pan}
                    onChange={(e) => update(t, { pan: Number(e.target.value) })}
                    onDoubleClick={() => update(t, { pan: 0 })}
                  />
                  <span className="small mono">
                    {Math.abs(t.pan) < 0.01 ? 'C' : `${t.pan < 0 ? 'I' : 'D'}${Math.round(Math.abs(t.pan) * 100)}`}
                  </span>
                </label>
              </div>
              <ClipLane
                track={t}
                duration={duration}
                sampleRate={project.sample_rate}
                color={kind.color}
                dimmed={!audible}
                selected={selected?.trackId === t.id ? selected.clipId : null}
                locked={recording}
                snap={snap}
                onSelect={(clipId) => setSelected(clipId ? { trackId: t.id, clipId } : null)}
                onSeek={seek}
                onPreview={(clips) => {
                  setFrozenDuration((d) => d ?? duration * 1.25)
                  onClipsChange(t.id, clips)
                }}
                onCommit={(before, after) => {
                  setFrozenDuration(null)
                  commitEdit([{ kind: 'clips', trackId: t.id, before, after }])
                }}
              />
            </div>
            {expanded.has(t.id) && automationRows(t.id, t.automation, t.effects, kind.color)}
            {shownNotes.has(t.id) && (trackNotes(t.id) || pitchFor(t.id)) && (
              <div className="lane notes-lane">
                <div className="lane-header auto-header">
                  <span className="lane-chip" style={{ background: kind.color }} />
                  <span className="small">
                    {trackNotes(t.id) ? tr('Notas · {n}', { n: trackNotes(t.id)!.notes.length }) : tr('Curva de tono')}
                    {pitchFor(t.id) && trackNotes(t.id) ? tr(' · curva de tono') : ''}
                  </span>
                  <button className="icon-btn" title={tr('Ocultar')} onClick={() => toggleNotes(t.id)}>
                    <Icon name="x" size={14} />
                  </button>
                </div>
                <NotesLane
                  notes={trackNotes(t.id)?.notes ?? []}
                  curve={pitchFor(t.id)}
                  duration={duration}
                  color={kind.color}
                  notation={notation}
                  drums={trackNotes(t.id)?.mode === 'drums'}
                  locked={recording}
                  snap={snap}
                  onSeek={seek}
                  onPreview={(notes) => setTrackNotes(t.id, notes)}
                  onCommit={(before, after) => commitEdit([{ kind: 'notes', trackId: t.id, before, after }])}
                />
              </div>
            )}
            </Fragment>
          )
        })}

        <div className="lane master-lane">
          <div className="lane-header compact-header">
            <strong>{tr('Máster')}</strong>
            <button
              className={`toggle auto ${expanded.has(MASTER_KEY) ? 'open' : ''} ${master.automation.some((l) => l.enabled && l.points.length) ? 'on' : ''}`}
              title={tr('Automatización del máster (volumen y efectos del bus)')}
              onClick={() => toggleExpanded(MASTER_KEY)}
            >
              A
            </button>
          </div>
          <div className="lane-wave" onPointerDown={(e) => seek((e.nativeEvent.offsetX / e.currentTarget.clientWidth) * duration)} />
        </div>
        {expanded.has(MASTER_KEY) && automationRows(null, master.automation, master.effects, '#e8eaf1')}

        <div className="playhead-layer">
          {loopView && (
            <div
              className={`loop-region ${loopView.on ? 'on' : ''}`}
              style={{ left: `${(loopView.a / duration) * 100}%`, width: `${((loopView.b - loopView.a) / duration) * 100}%` }}
            />
          )}
          {recording && recorder.startedAt != null && !transport.counting_in && position > recorder.startedAt && (
            <div
              className="record-region"
              style={{
                left: `${(recorder.startedAt / duration) * 100}%`,
                width: `${((position - recorder.startedAt) / duration) * 100}%`,
              }}
            />
          )}
          {practice.markers.map((m) => (
            <div key={m.id} className="marker-line" style={{ left: `${(m.time / duration) * 100}%` }}>
              <span>{m.name}</span>
            </div>
          ))}
          <div className="playhead" style={{ left: playheadPct }} />
        </div>
      </div>
      </div>

      <p className="hint muted small">
        {tr('Clic: saltar a esa posición. Arrastra un clip para moverlo, sus bordes para recortarlo y los tiradores de arriba para los fundidos. S divide en el cabezal, Supr borra el clip, Ctrl+Z deshace. Alt desactiva el ajuste a pulsos.')}
      </p>
    </section>
  )
}
