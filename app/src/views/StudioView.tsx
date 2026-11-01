import { useMemo, useRef, useState } from 'react'
import {
  api,
  errorMessage,
  type Analysis,
  type EffectSlot,
  type Lyrics,
  type MasterSettings,
  type PracticeSettings,
  type ProjectView,
  type SeparationResult,
  type TrackInfo,
  type TrackView,
} from '../lib/api'
import { PracticePanel } from '../components/PracticePanel'
import type { SeparationJob } from '../lib/useSeparation'
import { SeparationPanel } from '../components/SeparationPanel'
import { ExportPanel } from '../components/ExportPanel'
import { Icon } from '../components/Icon'
import { formatTime } from '../lib/format'
import { TRACK_KINDS } from '../lib/tracks'
import { Waveform } from '../components/Waveform'
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
  position: number
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

type Panel = 'separate' | 'export' | 'practice' | 'analysis' | 'lyrics' | 'mix' | 'master'

const HEADER_W = 280

export function StudioView({
  project,
  position,
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
  const practice = project.project.practice
  const hasStems = project.tracks.some((t) => t.kind !== 'mix')
  const analysis = project.analysis
  const [panel, setPanel] = useState<Panel | null>(hasStems ? (analysis ? 'analysis' : 'practice') : 'separate')
  const shownPanel = separation.job
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
  const duration = useMemo(() => Math.max(0.001, ...project.tracks.map((t) => t.duration)), [project])
  const anySolo = project.tracks.some((t) => t.solo)
  const lanesRef = useRef<HTMLDivElement>(null)

  const timeAt = (clientX: number): number | null => {
    const el = lanesRef.current
    if (!el) return null
    const rect = el.getBoundingClientRect()
    const x = clientX - rect.left - HEADER_W
    if (x < 0) return null
    return Math.max(0, Math.min(1, x / (rect.width - HEADER_W))) * duration
  }

  const seek = (t: number) => api.seek(t).catch((err) => onError(errorMessage(err)))

  const seekFromEvent = (e: React.MouseEvent) => {
    const t = timeAt(e.clientX)
    if (t != null) seek(t)
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

  const ticks = useMemo(() => {
    const step = duration > 600 ? 60 : duration > 240 ? 30 : duration > 60 ? 10 : 5
    const out: number[] = []
    for (let s = 0; s <= duration; s += step) out.push(s)
    return out
  }, [duration])

  const playheadPct = `${Math.min(100, (position / duration) * 100)}%`

  return (
    <section className="studio">
      <header className="view-header">
        <div>
          <h1>{project.project.name}</h1>
          <p className="muted">
            {project.tracks.length} {project.tracks.length === 1 ? 'pista' : 'pistas'} · {formatTime(duration)} ·{' '}
            {project.sample_rate / 1000} kHz
          </p>
        </div>
        <div className="toolbar">
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

      <div className="lanes" ref={lanesRef} style={{ ['--header-w' as string]: `${HEADER_W}px` }}>
        <div className="ruler" onMouseDown={onRulerDown}>
          <div className="ruler-spacer" />
          <div className="ruler-track">
            {ticks.map((s) => (
              <span key={s} className="tick" style={{ left: `${(s / duration) * 100}%` }}>
                {formatTime(s).slice(0, -3)}
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
              <div className="lane-wave" onMouseDown={seekFromEvent}>
                <Waveform
                  peaks={t.peaks}
                  timelineDuration={duration}
                  sampleRate={project.sample_rate}
                  color={kind.color}
                  dimmed={!audible}
                />
              </div>
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
          {practice.markers.map((m) => (
            <div key={m.id} className="marker-line" style={{ left: `${(m.time / duration) * 100}%` }}>
              <span>{m.name}</span>
            </div>
          ))}
          <div className="playhead" style={{ left: playheadPct }} />
        </div>
      </div>

      <p className="hint muted small">
        Haz clic en la forma de onda para saltar a esa posición. Barra espaciadora: reproducir o pausar.
      </p>
    </section>
  )
}
