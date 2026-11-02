import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { ask } from '@tauri-apps/plugin-dialog'
import {
  api,
  errorMessage,
  timelineEnd,
  type Analysis,
  type Clip,
  type EffectSlot,
  type Lyrics,
  type MasterSettings,
  type PracticeSettings,
  type ProjectView,
  type SeparationResult,
  type TrackInfo,
  type TrackView,
  type TransportEvent,
} from '../lib/api'
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

interface Props {
  project: ProjectView
  transport: TransportEvent
  recorder: Recorder
  onTrackRemoved: (trackId: string) => void
  onClipsChange: (trackId: string, clips: Clip[]) => void
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
}

type Panel = 'separate' | 'export' | 'practice' | 'analysis' | 'lyrics' | 'mix' | 'master' | 'record'

const HEADER_W = 280
const MAX_ZOOM = 64
/** Separaciones posibles entre marcas de la regla (s). */
const TICK_STEPS = [0.1, 0.25, 0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300]

/** Cambio de clips en una o varias pistas (una entrada del historial). */
type Edit = { trackId: string; before: Clip[]; after: Clip[] }[]

export function StudioView({
  project,
  transport,
  recorder,
  onTrackRemoved,
  onClipsChange,
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
          : panel
  const [notation, setNotation] = usePref<Notation>('notation', 'english', ['english', 'latin'])
  const [instrument, setInstrument] = usePref<'guitar' | 'piano'>('instrument', 'guitar', ['guitar', 'piano'])

  const hasVocalsStem = project.tracks.some((t) => t.file.startsWith('audio/stems/') && /\/vocals\.\w+$/.test(t.file))
  const [mixChannel, setMixChannel] = useState<string | null>(null)
  const master = project.project.master
  const setTrackEffects = (trackId: string, effects: EffectSlot[]) => {
    const t = project.tracks.find((x) => x.id === trackId)
    if (!t) return
    onTrackChange({ ...t, effects })
    api.effectsSet(trackId, effects).catch((err) => onError(errorMessage(err)))
  }
  const setMasterEffects = (effects: EffectSlot[]) => {
    onMasterChange({ ...master, effects })
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
  const commitEdit = useCallback(
    (edit: Edit) => {
      const changed = edit.filter((e) => JSON.stringify(e.before) !== JSON.stringify(e.after))
      if (changed.length === 0) return
      undoStack.current.push(changed)
      if (undoStack.current.length > 200) undoStack.current.shift()
      redoStack.current = []
      syncHistory()
      changed.forEach((e) => saveClips(e.trackId, e.after))
    },
    [saveClips],
  )
  const undo = useCallback(() => {
    const e = undoStack.current.pop()
    if (!e) return
    redoStack.current.push(e)
    syncHistory()
    e.forEach((c) => saveClips(c.trackId, c.before))
  }, [saveClips])
  const redo = useCallback(() => {
    const e = redoStack.current.pop()
    if (!e) return
    undoStack.current.push(e)
    syncHistory()
    e.forEach((c) => saveClips(c.trackId, c.after))
  }, [saveClips])

  /** Divide en el cabezal el clip seleccionado o, sin selección, todos los clips bajo el cabezal. */
  const splitAtPlayhead = useCallback(() => {
    const edit: Edit = []
    for (const t of project.tracks) {
      if (selected && selected.trackId !== t.id) continue
      const after = splitAt(t.clips, position, selected?.clipId)
      if (after) edit.push({ trackId: t.id, before: t.clips, after })
    }
    commitEdit(edit)
  }, [project.tracks, selected, position, commitEdit])

  const deleteSelected = useCallback(() => {
    if (!selected) return
    const t = project.tracks.find((x) => x.id === selected.trackId)
    if (!t) return
    commitEdit([{ trackId: t.id, before: t.clips, after: t.clips.filter((c) => c.id !== selected.clipId) }])
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

  const removeTrack = async (t: TrackView) => {
    const ok = await ask(`¿Eliminar «${t.name}»? El audio de la toma se borrará del proyecto.`, {
      title: 'Eliminar pista',
      kind: 'warning',
      okLabel: 'Eliminar',
      cancelLabel: 'Cancelar',
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
            {project.tracks.length} {project.tracks.length === 1 ? 'pista' : 'pistas'} · {formatTime(tracksEnd)} ·{' '}
            {project.sample_rate / 1000} kHz
          </p>
        </div>
        <div className="toolbar">
          <button
            className={`btn ${shownPanel === 'record' ? 'active' : ''}`}
            onClick={() => setPanel(panel === 'record' ? null : 'record')}
          >
            <Icon name="mic" size={16} /> Grabar
          </button>
          <button
            className={`btn ${shownPanel === 'separate' ? 'active' : ''}`}
            onClick={() => setPanel(panel === 'separate' ? null : 'separate')}
          >
            <Icon name="model" size={16} /> {hasStems ? 'Volver a separar' : 'Separar stems'}
          </button>
          <button
            className={`btn ${shownPanel === 'analysis' ? 'active' : ''}`}
            onClick={() => setPanel(panel === 'analysis' ? null : 'analysis')}
          >
            <Icon name="chord" size={16} /> Análisis
          </button>
          <button
            className={`btn ${shownPanel === 'lyrics' ? 'active' : ''}`}
            onClick={() => setPanel(panel === 'lyrics' ? null : 'lyrics')}
          >
            <Icon name="lyrics" size={16} /> Letra
          </button>
          <button
            className={`btn ${shownPanel === 'mix' ? 'active' : ''}`}
            onClick={() => setPanel(panel === 'mix' ? null : 'mix')}
          >
            <Icon name="sliders" size={16} /> Mezcla
          </button>
          <button
            className={`btn ${shownPanel === 'master' ? 'active' : ''}`}
            onClick={() => setPanel(panel === 'master' ? null : 'master')}
          >
            <Icon name="star" size={16} /> Máster
          </button>
          <button
            className={`btn ${shownPanel === 'practice' ? 'active' : ''}`}
            onClick={() => setPanel(panel === 'practice' ? null : 'practice')}
          >
            <Icon name="music" size={16} /> Práctica
          </button>
          <button
            className={`btn ${shownPanel === 'export' ? 'active' : ''}`}
            onClick={() => setPanel(panel === 'export' ? null : 'export')}
          >
            <Icon name="download" size={16} /> Exportar
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
        <button className="btn ghost small" onClick={undo} disabled={history.undo === 0 || recording} title="Deshacer (Ctrl+Z)">
          <Icon name="undo" size={15} /> Deshacer
        </button>
        <button className="btn ghost small" onClick={redo} disabled={history.redo === 0 || recording} title="Rehacer (Ctrl+Mayús+Z)">
          <Icon name="redo" size={15} /> Rehacer
        </button>
        <button
          className="btn ghost small"
          onClick={splitAtPlayhead}
          disabled={recording}
          title="Dividir en el cabezal el clip seleccionado o, sin selección, todos los clips bajo el cabezal (S)"
        >
          <Icon name="scissors" size={15} /> Dividir
        </button>
        <button className="btn ghost small" onClick={deleteSelected} disabled={!selectedTrack || recording} title="Eliminar el clip seleccionado (Supr)">
          <Icon name="trash" size={15} /> Eliminar clip
        </button>
        <label className="check small" title="Al mover o recortar, los bordes se pegan a los pulsos (mantén Alt para moverlos libremente)">
          <input type="checkbox" checked={snapOn === 'on'} onChange={(e) => setSnapOn(e.target.checked ? 'on' : 'off')} />
          Ajustar a pulsos
        </label>
        <div className="zoom">
          <button className="icon-btn small" onClick={() => setZoom(zoom / 1.5)} disabled={zoom <= 1} title="Alejar">
            −
          </button>
          <input
            type="range"
            min={0}
            max={Math.log2(MAX_ZOOM)}
            step={0.05}
            value={Math.log2(zoom)}
            onChange={(e) => setZoom(2 ** Number(e.target.value))}
            aria-label="Zoom"
            title="Zoom (Ctrl + rueda)"
          />
          <button className="icon-btn small" onClick={() => setZoom(zoom * 1.5)} disabled={zoom >= MAX_ZOOM} title="Acercar">
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
          return (
            <div key={t.id} className="lane">
              <div className="lane-header">
                <span className="lane-chip" style={{ background: kind.color }} />
                <div className="lane-title">
                  <strong title={t.name}>{t.name}</strong>
                  <span className="muted small">{kind.label}</span>
                </div>
                <div className="lane-buttons">
                  <button
                    className={`toggle mute ${t.mute ? 'on' : ''}`}
                    title="Silenciar (M)"
                    onClick={() => update(t, { mute: !t.mute })}
                  >
                    M
                  </button>
                  <button
                    className={`toggle solo ${t.solo ? 'on' : ''}`}
                    title="Solo (S)"
                    onClick={() => update(t, { solo: !t.solo })}
                  >
                    S
                  </button>
                  <button
                    className={`toggle fx ${t.effects.some((e) => e.enabled) ? 'on' : ''}`}
                    title={t.effects.length ? `Efectos: ${t.effects.length}` : 'Añadir efectos'}
                    onClick={() => openFx(t.id)}
                  >
                    FX
                  </button>
                  {t.kind === 'recording' && (
                    <button
                      className="icon-btn lane-remove"
                      title="Eliminar la toma"
                      disabled={recording}
                      onClick={() => removeTrack(t)}
                    >
                      <Icon name="trash" size={14} />
                    </button>
                  )}
                </div>
                <label className="lane-slider" title={`Volumen: ${t.gain_db.toFixed(1)} dB (doble clic: 0 dB)`}>
                  <span className="small muted">Vol</span>
                  <input
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
                <label className="lane-slider" title="Panorama (doble clic: centro)">
                  <span className="small muted">Pan</span>
                  <input
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
                  commitEdit([{ trackId: t.id, before, after }])
                }}
              />
            </div>
          )
        })}

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
        Clic: saltar a esa posición. Arrastra un clip para moverlo, sus bordes para recortarlo y los tiradores de
        arriba para los fundidos. S divide en el cabezal, Supr borra el clip, Ctrl+Z deshace. Alt desactiva el ajuste a
        pulsos.
      </p>
    </section>
  )
}
