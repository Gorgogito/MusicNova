import { useEffect, useState } from 'react'
import { api, errorMessage, type Analysis, type ModelInfo, type PracticeSettings } from '../lib/api'
import { chordIndexAt, chordName, chordShift, keyName, mod12, prefersFlats, type Notation } from '../lib/chords'
import type { AnalysisJob } from '../lib/useAnalysis'
import { ChordDiagram } from './ChordDiagram'
import { Icon } from './Icon'

interface Props {
  analysis: Analysis | null
  practice: PracticeSettings
  position: number
  job: AnalysisJob | null
  workerReady: boolean
  notation: Notation
  instrument: 'guitar' | 'piano'
  onNotation: (n: Notation) => void
  onInstrument: (i: 'guitar' | 'piano') => void
  onStart: (device: string, beatModelId?: string) => void
  onCancel: () => void
  onPracticeChange: (patch: Partial<PracticeSettings>) => void
  onOpenModels: () => void
  onError: (m: string) => void
}

export function AnalysisPanel(props: Props) {
  const { analysis, practice, position, job, workerReady, notation, instrument } = props
  const [models, setModels] = useState<ModelInfo[] | null>(null)
  const [beatModel, setBeatModel] = useState('')
  const [device, setDevice] = useState('auto')
  const [showSetup, setShowSetup] = useState(false)
  const [hasBtc, setHasBtc] = useState(true)

  useEffect(() => {
    if (!workerReady) return
    api
      .worker<{ models: ModelInfo[] }>('models.list')
      .then((r) => {
        const installed = r.models
          .filter((m) => m.task === 'beats' && m.installed)
          .sort((a, b) => Number(b.priority ?? 0) - Number(a.priority ?? 0))
        setModels(installed)
        setHasBtc(r.models.some((m) => m.task === 'chords' && m.installed))
        setBeatModel((cur) => (installed.some((m) => m.id === cur) ? cur : (installed[0]?.id ?? 'none')))
      })
      .catch((e) => props.onError(errorMessage(e)))
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workerReady])

  if (job) {
    return (
      <div className="panel">
        <div className="panel-row">
          <strong>Analizando la canción</strong>
          <span className="muted small">{job.message}</span>
          <button className="btn ghost small" onClick={props.onCancel} disabled={!job.jobId}>
            <Icon name="x" size={14} /> Cancelar
          </button>
        </div>
        <div className="progress">
          <div style={{ width: `${job.progress * 100}%` }} />
        </div>
      </div>
    )
  }

  const setup = (
    <div className="panel-row">
      <label className="field">
        <span className="small muted">Detector de pulsos</span>
        <select value={beatModel} onChange={(e) => setBeatModel(e.target.value)}>
          {models?.map((m) => (
            <option key={m.id} value={m.id}>
              {m.name}
            </option>
          ))}
          <option value="none">Básico (sin modelo, menos preciso)</option>
        </select>
      </label>
      <label className="field">
        <span className="small muted">Procesar con</span>
        <select value={device} onChange={(e) => setDevice(e.target.value)}>
          <option value="auto">Automático (GPU si está disponible)</option>
          <option value="cpu">CPU</option>
        </select>
      </label>
      <button className="btn primary" disabled={!workerReady} onClick={() => props.onStart(device, beatModel || undefined)}>
        <Icon name="music" size={16} /> {analysis ? 'Volver a analizar' : 'Analizar'}
      </button>
    </div>
  )

  if (!analysis || showSetup) {
    return (
      <div className="panel">
        {!workerReady && <span className="muted">El proceso de IA se está iniciando…</span>}
        <p className="small muted">
          Detecta pulsos, compás, tempo, tonalidad, acordes y secciones.
        </p>
        {models && models.length === 0 && (
          <div className="notice small">
            Para pulsos y compases precisos descarga <strong>Beat This!</strong> (MIT, 8–80 MB).{' '}
            <button className="link small" onClick={props.onOpenModels}>
              Ver modelos
            </button>
          </div>
        )}
        {!hasBtc && (
          <div className="notice small">
            Para acordes más precisos (14 tipos: 7, maj7, m7♭5, dim7, sus2, 6…) descarga <strong>BTC</strong> (MIT, 12 MB):
            con guitarra real acierta el tipo de acorde un 62 % del tiempo frente al 41 % sin él.{' '}
            <button className="link small" onClick={props.onOpenModels}>
              Ver modelos
            </button>
          </div>
        )}
        {setup}
        {analysis && (
          <button className="link small" onClick={() => setShowSetup(false)}>
            Volver al resultado
          </button>
        )}
      </div>
    )
  }

  const shift = chordShift(practice)
  const soundingKey = mod12(analysis.key.tonic + Math.round(practice.pitch))
  const flats = prefersFlats(mod12(analysis.key.tonic + shift), analysis.key.mode)
  const i = chordIndexAt(analysis.chords, position)
  const current = i >= 0 ? analysis.chords[i] : null
  const next = analysis.chords.slice(i + 1).find((c) => c.root != null && c.label !== current?.label) ?? null
  const shown = (c: typeof current) =>
    c && c.root != null ? chordName(mod12(c.root + shift), c.quality, notation, flats) : 'N.C.'
  const detected = practice.beat_source === 'detected'

  return (
    <div className="analysis">
      <section className="practice-group analysis-now">
        <header>
          <strong>Acorde actual</strong>
          <div className="seg small">
            <button className={instrument === 'guitar' ? 'on' : ''} onClick={() => props.onInstrument('guitar')}>
              Guitarra
            </button>
            <button className={instrument === 'piano' ? 'on' : ''} onClick={() => props.onInstrument('piano')}>
              Piano
            </button>
          </div>
        </header>
        <div className="now-row">
          <div className="now-names">
            <span className="chord-big">{shown(current)}</span>
            <span className="muted small">después: {next ? shown(next) : '—'}</span>
          </div>
          {current?.root != null && current.quality != null && (
            <ChordDiagram root={mod12(current.root + shift)} quality={current.quality} instrument={instrument} />
          )}
        </div>
      </section>

      <section className="practice-group">
        <header>
          <strong>Tonalidad</strong>
          <span className="mono">{keyName(soundingKey, analysis.key.mode, notation)}</span>
        </header>
        <div className="confidence" title="Confianza de la estimación">
          <div style={{ width: `${Math.round(analysis.key.confidence * 100)}%` }} />
        </div>
        <span className="small muted">
          También posible:{' '}
          {analysis.key.alternatives
            .slice(0, 2)
            .map((k) => keyName(mod12(k.tonic + Math.round(practice.pitch)), k.mode, notation))
            .join(' · ')}
        </span>
        {practice.pitch !== 0 && (
          <span className="small muted">Original: {keyName(analysis.key.tonic, analysis.key.mode, notation)}</span>
        )}
      </section>

      <section className="practice-group">
        <header>
          <strong>Tempo y compás</strong>
          <span className="mono">
            {(analysis.bpm * practice.tempo).toFixed(1)} BPM · {analysis.beats_per_bar}/4
          </span>
        </header>
        <span className="small muted">
          {analysis.tempo_stable ? 'Tempo estable' : 'Tempo variable: el metrónomo sigue los pulsos reales'}
          {practice.tempo !== 1 && ` · original ${analysis.bpm.toFixed(1)} BPM`}
        </span>
        <label className="inline small">
          Metrónomo sobre
          <select
            className="compact"
            value={practice.beat_source}
            onChange={(e) => props.onPracticeChange({ beat_source: e.target.value as PracticeSettings['beat_source'] })}
          >
            <option value="detected">pulsos detectados</option>
            <option value="manual">tempo manual</option>
          </select>
        </label>
        {!detected && <span className="small muted">Ajusta el BPM manual en Práctica.</span>}
      </section>

      <section className="practice-group">
        <header>
          <strong>Acordes</strong>
          <div className="seg small">
            <button className={notation === 'english' ? 'on' : ''} onClick={() => props.onNotation('english')}>
              C D E
            </button>
            <button className={notation === 'latin' ? 'on' : ''} onClick={() => props.onNotation('latin')}>
              Do Re Mi
            </button>
          </div>
        </header>
        <label className="inline small">
          Cejilla
          <select
            className="compact"
            value={practice.capo}
            onChange={(e) => props.onPracticeChange({ capo: Number(e.target.value) })}
          >
            <option value={0}>Sin cejilla</option>
            {Array.from({ length: 12 }, (_, k) => (
              <option key={k + 1} value={k + 1}>
                Traste {k + 1}
              </option>
            ))}
          </select>
        </label>
        <span className="small muted">
          {practice.capo > 0 || practice.pitch !== 0
            ? `Acordes adaptados${practice.pitch !== 0 ? ` al tono (${practice.pitch > 0 ? '+' : ''}${practice.pitch} st)` : ''}${practice.capo > 0 ? ` y a la cejilla en el traste ${practice.capo}` : ''}.`
            : 'Doble clic en un acorde o sección de la línea de tiempo para corregirlo.'}
        </span>
        <span className="small muted">
          Acordes: {analysis.chord_source === 'btc' ? 'red neuronal BTC (mezcla completa)' : analysis.chord_source === 'stems' ? 'plantillas sobre los stems (sin batería ni voz)' : 'plantillas sobre la mezcla'} · pulsos:{' '}
          {analysis.beat_source === 'librosa' ? 'básico' : analysis.beat_source}
        </span>
        <button className="link small" onClick={() => setShowSetup(true)}>
          Volver a analizar…
        </button>
      </section>
    </div>
  )
}
