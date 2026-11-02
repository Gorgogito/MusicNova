// Capa tipada sobre los comandos y eventos de Tauri.
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

export type TrackKind =
  | 'mix' | 'vocals' | 'backing_vocals' | 'drums' | 'bass' | 'guitar'
  | 'piano' | 'other' | 'generated' | 'recording' | 'instrument'

export interface Band {
  freq: number
  gain_db: number
  q: number
}

export type EffectParams =
  | { type: 'eq'; highpass: number; low_shelf: Band; mid1: Band; mid2: Band; high_shelf: Band; lowpass: number }
  | { type: 'compressor'; threshold_db: number; ratio: number; attack_ms: number; release_ms: number; knee_db: number; makeup_db: number }
  | { type: 'reverb'; room_size: number; damping: number; width: number; predelay_ms: number; mix: number }
  | { type: 'delay'; time_ms: number; feedback: number; high_cut: number; ping_pong: boolean; mix: number }
  | { type: 'limiter'; input_db: number; ceiling_db: number; release_ms: number; true_peak: boolean }
  | { type: 'plugin' }

/** Efectos propios (los plugins externos van aparte). */
export type EffectType = Exclude<EffectParams['type'], 'plugin'>

export interface EffectSlot {
  id: string
  enabled: boolean
  params: EffectParams
  /** Plugin externo (solo si `params.type` es 'plugin'). */
  plugin?: PluginRef
  /** Pista que controla el efecto (sidechain del compresor). */
  sidechain?: string | null
}

export interface PluginRef {
  format: 'clap' | 'vst3'
  path: string
  id: string
  name: string
  vendor: string
  state?: string | null
}

export interface PluginDescriptor {
  format: 'clap' | 'vst3'
  path: string
  id: string
  name: string
  vendor: string
  version: string
  category: string
  /** Instrumento (recibe notas) en vez de efecto. */
  instrument?: boolean
}

export interface PluginCache {
  folders: string[]
  plugins: PluginDescriptor[]
  failed: { path: string; error: string }[]
  scanned_at: string | null
  default_folders: string[]
}

export interface PluginParam {
  id: number
  name: string
  min: number
  max: number
  default: number
  value: number
  stepped: boolean
  text: string
}

export interface LoudnessSummary {
  lufs: number | null
  true_peak_db: number | null
  lra: number
}

export interface AutoMasterReport {
  master: MasterSettings
  before: LoudnessSummary
  after: LoudnessSummary
  reached: boolean
  passes: number
}

export interface MasterReferenceResult {
  output: string
  reference: string
  warnings: string[]
  seconds: number
}

export interface MasterSettings {
  gain_db: number
  effects: EffectSlot[]
  automation: AutomationLane[]
}

export type AutomationTarget =
  | { kind: 'volume' }
  | { kind: 'pan' }
  | { kind: 'effect'; slot: string; param: string }

export interface AutoPoint {
  time: number
  value: number
}

export interface AutomationLane {
  id: string
  target: AutomationTarget
  enabled: boolean
  points: AutoPoint[]
}

export interface TrackInfo {
  id: string
  name: string
  kind: TrackKind
  file: string
  gain_db: number
  pan: number
  mute: boolean
  solo: boolean
  effects: EffectSlot[]
  /** Formato antiguo; al abrir el proyecto se convierte en un clip. */
  offset: number
  clips: Clip[]
  automation: AutomationLane[]
  /** Con qué suena una pista de instrumento. */
  instrument?: InstrumentSource | null
}

/** Timbres del sintetizador integrado. */
export type SynthTimbre = 'keys' | 'bass' | 'lead' | 'drums' | 'pad' | 'pluck'

export type InstrumentSource =
  | { type: 'synth'; timbre: SynthTimbre }
  | { type: 'plugin'; plugin: PluginRef }

/** Tramo del audio de una pista en la línea de tiempo (edición no destructiva). */
export interface Clip {
  id: string
  /** Segundo de la línea de tiempo en el que empieza. */
  start: number
  /** Segundo del archivo de la pista en el que empieza. */
  source_start: number
  length: number
  fade_in: number
  fade_out: number
  gain_db: number
}

export interface Peaks {
  frames_per_peak: number
  data: number[]
}

export interface TrackView extends TrackInfo {
  duration: number
  peaks: Peaks
}

export interface Marker {
  id: string
  name: string
  time: number
}

export interface PracticeSettings {
  tempo: number
  pitch: number
  loop_enabled: boolean
  loop_start: number
  loop_end: number
  metronome: boolean
  metronome_gain_db: number
  bpm: number
  beat_offset: number
  beats_per_bar: number
  count_in_bars: number
  markers: Marker[]
  trainer_enabled: boolean
  trainer_start: number
  trainer_step: number
  trainer_target: number
  trainer_every: number
  beat_source: 'manual' | 'detected'
  capo: number
}

export interface MusicalKey {
  tonic: number
  mode: 'major' | 'minor'
  label: string
  confidence: number
  alternatives: { tonic: number; mode: 'major' | 'minor'; label: string }[]
}

export interface Chord {
  start: number
  end: number
  /** Notación inglesa ("Am7") o "N" sin acorde. */
  label: string
  root: number | null
  quality: string | null
}

export interface Section {
  start: number
  end: number
  label: string
}

export interface LyricWord {
  start: number
  end: number
  text: string
  prob: number
}

export interface LyricLine {
  start: number
  end: number
  text: string
  words: LyricWord[]
}

export interface Lyrics {
  version: number
  language: string
  language_probability: number
  model_id: string
  source: 'vocals' | 'mix'
  duration: number
  lines: LyricLine[]
  seconds: number
  device: string
  warning: string | null
}

export type LyricsFormat = 'lrc' | 'lrc_words' | 'srt' | 'txt'

export interface Analysis {
  version: number
  duration: number
  beat_source: string
  chord_source: 'stems' | 'mix' | 'btc'
  bpm: number
  tempo_stable: boolean
  tempo_curve: [number, number][]
  beats_per_bar: number
  beats: number[]
  downbeats: number[]
  key: MusicalKey
  chords: Chord[]
  sections: Section[]
  seconds: number
  device: string
}

export interface Project {
  format_version: number
  id: string
  name: string
  created: string
  modified: string
  source_path: string | null
  tracks: TrackInfo[]
  practice: PracticeSettings
  master: MasterSettings
}

export interface ProjectView {
  dir: string
  project: Project
  tracks: TrackView[]
  sample_rate: number
  analysis: Analysis | null
  lyrics: Lyrics | null
  plugin_errors: string[]
  notes: Transcription | null
  pitch: PitchCurve | null
  instruments: InstrumentReport | null
}

// ---- Fase 6: transcripción ----------------------------------------------------------

export interface Note {
  /** Segundos de la línea de tiempo. */
  start: number
  end: number
  /** Nota MIDI (60 = Do central). */
  pitch: number
  velocity: number
  /** Curva de tono dentro de la nota: [segundos desde el inicio, cents]. */
  bends?: [number, number][]
}

/** «poly» (acordes), «mono» (una nota a la vez) o «drums» (percusión General MIDI). */
export type NotesMode = 'poly' | 'mono' | 'drums'

export interface TrackNotes {
  track_id: string
  mode: NotesMode
  notes: Note[]
}

export interface Transcription {
  version: number
  tracks: TrackNotes[]
}

/** Reducción de ruido constante (espectral, sin IA). */
export interface DenoiseSettings {
  /** Atenuación máxima del ruido en dB. */
  reduction_db: number
  /** 0–1: cuánto ruido se supone (0,5 = el estimado). */
  sensitivity: number
  /** Tramo (s) con solo ruido para tomar el perfil. */
  noise_region: [number, number] | null
}

/** Pistas que sabe generar ACE-Step. */
export type GeneratedTrack =
  | 'drums'
  | 'bass'
  | 'guitar'
  | 'keyboard'
  | 'strings'
  | 'synth'
  | 'percussion'
  | 'brass'
  | 'woodwinds'
  | 'backing_vocals'
  | 'vocals'
  | 'fx'

/** Detector de tono de la voz: CREPE (red neuronal, completo o rápido) o pYIN. */
export type PitchEngine = 'crepe-full' | 'crepe-tiny' | 'pyin'

export interface PitchCurve {
  version: number
  track_id: string | null
  source: string
  /** Intervalo entre puntos (s); el punto k es el instante k·hop. */
  hop: number
  /** Hz; 0 = sin voz. */
  f0: number[]
  confidence: number[]
}

export interface Instrument {
  id: string
  label: string
  confidence: number
  coverage: number
  segments: [number, number][]
}

export interface InstrumentReport {
  version: number
  source: string
  duration: number
  instruments: Instrument[]
  /** Stems separados con los que se analizó (vacío = solo la mezcla). */
  stems?: string[]
}

export interface TuneSettings {
  strength: number
  speed: number
  /** Clases de altura permitidas (0 = Do); vacío = cromática. */
  scale: number[]
  preserve_formants: boolean
}

export interface ProjectSummary {
  dir: string
  name: string
  modified: string
  tracks: number
}

export interface AppInfo {
  version: string
  sample_rate: number | null
  device: string | null
  engine_error: string | null
  projects_root: string
  supported_extensions: string[]
  ffmpeg_available: boolean
}

export interface TransportEvent {
  position: number
  duration: number
  playing: boolean
  counting_in: boolean
  loop_count: number
  peak_l: number
  peak_r: number
  lufs_m: number | null
  lufs_s: number | null
  recording: boolean
  /** Picos de la entrada abierta; `null` si no hay entrada activa. */
  input_peak: [number, number] | null
  /** Segundos grabados en la toma actual. */
  recorded: number
}

export interface InputDeviceInfo {
  name: string
  channels: number
  sample_rate: number
  is_default: boolean
}

export type InputChannels = { mode: 'mono' | 'stereo'; first: number }

export interface InputInfo {
  device: string
  sample_rate: number
  device_channels: number
  channels: InputChannels
  monitor_available: boolean
}

export interface LatencyReport {
  latency_ms: number
  spread_ms: number
  detected: number
  total: number
  level_db: number
}

export interface RecordFinished {
  track: TrackView | null
  message: string | null
}

export interface WorkerStatus {
  state: 'stopped' | 'starting' | 'ready' | 'crashed'
  python: string
  worker_dir: string
  error: string | null
  stderr_tail: string[]
}

export interface ModelInfo {
  id: string
  name: string
  task: string
  family: string
  description: string
  license: string
  license_url: string
  commercial_use: boolean
  size_bytes: number | null
  installed: boolean
  path: string
  stems?: string[]
  min_vram_gb?: number
  priority?: number
}

export type ExportFormat =
  | 'wav16' | 'wav24' | 'wav32f' | 'flac16' | 'flac24' | 'mp3_320' | 'mp3_256' | 'mp3_192' | 'mp3_128'

export interface StemOutput {
  name: string
  label: string
  file: string
}

export interface SeparationResult {
  model_id: string
  sample_rate: number
  device: string
  seconds: number
  duration: number
  stems: StemOutput[]
}

export interface GpuInfo {
  name: string
  vram_bytes: number | null
  driver?: string
  vendor?: string
}

export interface SystemInfo {
  python: string
  platform: string
  machine: string
  cpu_count: number
  ram_bytes: number | null
  gpus: GpuInfo[]
  torch: { version: string; cuda: boolean; cuda_version: string | null; mps: boolean; devices: GpuInfo[] } | null
  onnxruntime: { version: string; providers: string[] } | null
  data_dir: string
}

export type WorkerEvent =
  | { event: 'ready'; data: { version: string; protocol: number } }
  | { event: 'job.progress'; data: { job_id: string; kind: string; progress: number; message: string } }
  | { event: 'job.done'; data: { job_id: string; kind: string; result: unknown } }
  | { event: 'job.failed'; data: { job_id: string; kind: string; error: string } }
  | { event: 'job.cancelled'; data: { job_id: string; kind: string } }

export const api = {
  appInfo: () => invoke<AppInfo>('app_info'),
  importAudio: (path: string) => invoke<ProjectView>('import_audio', { path }),
  createProject: (name: string) => invoke<ProjectView>('create_project', { name }),
  openProject: (dir: string) => invoke<ProjectView>('open_project', { dir }),
  listProjects: () => invoke<ProjectSummary[]>('list_projects'),
  play: (countIn = true) => invoke<void>('transport_play', { countIn }),
  pause: () => invoke<void>('transport_pause'),
  seek: (seconds: number) => invoke<void>('transport_seek', { seconds }),
  updateTrack: (trackId: string, patch: Partial<Pick<TrackInfo, 'gain_db' | 'pan' | 'mute' | 'solo'>>) =>
    invoke<TrackInfo>('track_update', {
      trackId,
      gainDb: patch.gain_db,
      pan: patch.pan,
      mute: patch.mute,
      solo: patch.solo,
    }),
  worker: <T = unknown>(method: string, params?: Record<string, unknown>) =>
    invoke<T>('worker_call', { method, params }),
  workerStatus: () => invoke<WorkerStatus>('worker_status'),
  workerRestart: () => invoke<void>('worker_restart'),
  separateStart: (modelId: string, device?: string) =>
    invoke<{ job_id: string }>('separate_start', { modelId, device }),
  separateApply: (stems: StemOutput[]) => invoke<ProjectView>('separate_apply', { stems }),
  exportMix: (path: string, format: ExportFormat) =>
    invoke<{
      path: string
      peak_db: number
      sample_rate: number
      lufs: number | null
      true_peak_db: number | null
      lra: number
    }>('export_mix', { path, format }),
  practiceSet: (settings: PracticeSettings) => invoke<PracticeSettings>('practice_set', { settings }),
  exportStems: (dir: string, format: ExportFormat) => invoke<string[]>('export_stems', { dir, format }),
  analyzeStart: (device?: string, beatModelId?: string) =>
    invoke<{ job_id: string }>('analyze_start', { device, beatModelId }),
  analysisApply: () => invoke<{ analysis: Analysis; practice: PracticeSettings }>('analysis_apply'),
  analysisUpdate: (analysis: Analysis) => invoke<Analysis>('analysis_update', { analysis }),
  effectsSet: (trackId: string | null, effects: EffectSlot[]) =>
    invoke<EffectSlot[]>('effects_set', { trackId, effects }),
  masterGainSet: (gainDb: number) => invoke<number>('master_gain_set', { gainDb }),
  masterAuto: (targetLufs: number, ceilingDb: number) =>
    invoke<AutoMasterReport>('master_auto', { targetLufs, ceilingDb }),
  masterReferenceStart: (reference: string) => invoke<{ job_id: string }>('master_reference_start', { reference }),
  masterReferenceApply: (output: string, referenceName: string) =>
    invoke<ProjectView>('master_reference_apply', { output, referenceName }),
  lyricsStart: (modelId?: string, language?: string, device?: string) =>
    invoke<{ job_id: string }>('lyrics_start', { modelId, language, device }),
  lyricsApply: () => invoke<Lyrics>('lyrics_apply'),
  lyricsUpdate: (lyrics: Lyrics) => invoke<Lyrics>('lyrics_update', { lyrics }),
  lyricsExport: (path: string, format: LyricsFormat) => invoke<string>('lyrics_export', { path, format }),
  notesStart: (trackId: string, mode: NotesMode, sensitivity: number, device: string) =>
    invoke<{ job_id: string }>('notes_start', { trackId, mode, sensitivity, device }),
  notesApply: (trackId: string) => invoke<Transcription>('notes_apply', { trackId }),
  notesUpdate: (trackId: string, notes: Note[]) => invoke<Transcription>('notes_update', { trackId, notes }),
  notesRemove: (trackId: string) => invoke<Transcription>('notes_remove', { trackId }),
  notesExportMidi: (path: string, trackId: string | null) => invoke<string>('notes_export_midi', { path, trackId }),
  notesRender: (trackId: string) => invoke<ProjectView>('notes_render', { trackId }),
  pitchStart: (trackId: string, engine: PitchEngine) => invoke<{ job_id: string }>('pitch_start', { trackId, engine }),
  pitchApply: (trackId: string) => invoke<PitchCurve>('pitch_apply', { trackId }),
  tuneRender: (settings: TuneSettings) => invoke<ProjectView>('tune_render', { settings }),
  instrumentsStart: (modelId: string | undefined, device: string) =>
    invoke<{ job_id: string }>('instruments_start', { modelId, device }),
  instrumentsApply: () => invoke<InstrumentReport>('instruments_apply'),
  denoiseSteady: (trackId: string, settings: DenoiseSettings) => invoke<ProjectView>('denoise_steady', { trackId, settings }),
  denoiseVoiceStart: (trackId: string, attenLimDb: number | null) =>
    invoke<{ job_id: string }>('denoise_voice_start', { trackId, attenLimDb }),
  denoiseVoiceApply: (trackId: string) => invoke<ProjectView>('denoise_voice_apply', { trackId }),
  generateStart: (track: GeneratedTrack, caption: string, context: 'audible' | 'original', steps: number, seed: number | null) =>
    invoke<{ job_id: string }>('generate_start', { track, caption, context, steps, seed }),
  generateApply: (track: GeneratedTrack) => invoke<ProjectView>('generate_apply', { track }),
  instrumentTrackAdd: (name: string, source: InstrumentSource) => invoke<ProjectView>('instrument_track_add', { name, source }),
  instrumentSet: (trackId: string, source: InstrumentSource) => invoke<TrackView>('instrument_set', { trackId, source }),
  instrumentRender: (trackId: string) => invoke<TrackView>('instrument_render', { trackId }),
  midiImport: (path: string) => invoke<ProjectView>('midi_import', { path }),
  inputDevices: () => invoke<InputDeviceInfo[]>('input_devices'),
  inputOpen: (device: string | null, channels: InputChannels) => invoke<InputInfo>('input_open', { device, channels }),
  inputClose: () => invoke<void>('input_close'),
  inputMonitor: (enabled: boolean, gainDb: number) => invoke<void>('input_monitor', { enabled, gainDb }),
  recordStart: (latencyMs: number) => invoke<void>('record_start', { latencyMs }),
  recordStop: () => invoke<void>('record_stop'),
  recordCalibrate: () => invoke<LatencyReport>('record_calibrate'),
  trackRemove: (trackId: string) => invoke<void>('track_remove', { trackId }),
  clipsSet: (trackId: string, clips: Clip[]) => invoke<Clip[]>('clips_set', { trackId, clips }),
  pluginsList: () => invoke<PluginCache>('plugins_list'),
  pluginsScan: (folders: string[]) => invoke<PluginCache>('plugins_scan', { folders }),
  pluginOpenEditor: (slotId: string) => invoke<void>('plugin_open_editor', { slotId }),
  pluginParams: (slotId: string) => invoke<PluginParam[]>('plugin_params', { slotId }),
  pluginParamSet: (slotId: string, id: number, value: number) => invoke<void>('plugin_param_set', { slotId, id, value }),
  pluginsSync: () => invoke<void>('plugins_sync'),
  automationSet: (trackId: string | null, lanes: AutomationLane[]) =>
    invoke<AutomationLane[]>('automation_set', { trackId, lanes }),
}

export const events = {
  transport: (cb: (e: TransportEvent) => void): Promise<UnlistenFn> =>
    listen<TransportEvent>('transport', (e) => cb(e.payload)),
  worker: (cb: (e: WorkerEvent) => void): Promise<UnlistenFn> =>
    listen<WorkerEvent>('worker-event', (e) => cb(e.payload)),
  workerStatus: (cb: (s: WorkerStatus) => void): Promise<UnlistenFn> =>
    listen<WorkerStatus>('worker-status', (e) => cb(e.payload)),
  pluginScan: (cb: (p: { done: number; total: number; current: string }) => void): Promise<UnlistenFn> =>
    listen<{ done: number; total: number; current: string }>('plugin-scan', (e) => cb(e.payload)),
  recordFinished: (cb: (r: RecordFinished) => void): Promise<UnlistenFn> =>
    listen<RecordFinished>('record-finished', (e) => cb(e.payload)),
}

/** Final de la línea de tiempo: el clip que acaba más tarde. */
export function timelineEnd(tracks: { clips: Clip[]; offset: number; duration: number }[]): number {
  return Math.max(
    0,
    ...tracks.map((t) => (t.clips?.length ? Math.max(...t.clips.map((c) => c.start + c.length)) : (t.offset ?? 0) + t.duration)),
  )
}

export function errorMessage(e: unknown): string {
  return typeof e === 'string' ? e : e instanceof Error ? e.message : JSON.stringify(e)
}
