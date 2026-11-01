import { useEffect, useState } from 'react'
import { save } from '@tauri-apps/plugin-dialog'
import { api, errorMessage, type LyricLine, type Lyrics, type LyricsFormat, type ModelInfo } from '../lib/api'
import { activeLine, LANGUAGES, languageName, newLineAt, retextLine, shiftAll, shiftLine, wordProgress } from '../lib/lyrics'
import type { LyricsJob } from '../lib/useLyrics'
import { formatTime } from '../lib/format'
import { Icon } from './Icon'

interface Props {
  lyrics: Lyrics | null
  projectName: string
  hasVocalsStem: boolean
  position: number
  job: LyricsJob | null
  workerReady: boolean
  onStart: (modelId: string | undefined, language: string, device: string) => void
  onCancel: () => void
  onChange: (l: Lyrics) => void
  onSeek: (t: number) => void
  onOpenModels: () => void
  onError: (m: string) => void
}

const EXPORTS: { id: LyricsFormat; label: string; ext: string }[] = [
  { id: 'lrc', label: 'LRC (por línea)', ext: 'lrc' },
  { id: 'lrc_words', label: 'LRC karaoke (por palabra)', ext: 'lrc' },
  { id: 'srt', label: 'Subtítulos SRT', ext: 'srt' },
  { id: 'txt', label: 'Texto', ext: 'txt' },
]

export function LyricsPanel(props: Props) {
  const { lyrics, position, job, workerReady } = props
  const [models, setModels] = useState<ModelInfo[] | null>(null)
  const [modelId, setModelId] = useState('')
  const [language, setLanguage] = useState('auto')
  const [device, setDevice] = useState('auto')
  const [mode, setMode] = useState<'karaoke' | 'edit' | 'setup'>('karaoke')
  const [status, setStatus] = useState<string | null>(null)

  useEffect(() => {
    if (!workerReady) return
    api
      .worker<{ models: ModelInfo[] }>('models.list')
      .then((r) => {
        const installed = r.models
          .filter((m) => m.task === 'lyrics' && m.installed)
          .sort((a, b) => Number(b.priority ?? 0) - Number(a.priority ?? 0))
        setModels(installed)
        setModelId((cur) => (installed.some((m) => m.id === cur) ? cur : (installed[0]?.id ?? '')))
      })
      .catch((e) => props.onError(errorMessage(e)))
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workerReady])

  if (job) {
    return (
      <div className="panel">
        <div className="panel-row">
          <strong>Transcribiendo la letra</strong>
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

  if (!lyrics || mode === 'setup') {
    const noModels = models !== null && models.length === 0
    return (
      <div className="panel">
        {!workerReady && <span className="muted">El proceso de IA se está iniciando…</span>}
        <p className="small muted">
          Transcribe la letra con marcas de tiempo por palabra para cantar en modo karaoke y exportarla a LRC o SRT.
        </p>
        {!props.hasVocalsStem && (
          <div className="notice small">
            Consejo: separa primero los stems. Con la voz aislada la transcripción es mucho más fiable.
          </div>
        )}
        {noModels ? (
          <div className="panel-row">
            <span>Necesitas descargar un modelo Whisper (una sola vez).</span>
            <button className="btn primary" onClick={props.onOpenModels}>
              <Icon name="download" size={16} /> Ver modelos
            </button>
          </div>
        ) : (
          <div className="panel-row">
            <label className="field">
              <span className="small muted">Modelo</span>
              <select value={modelId} onChange={(e) => setModelId(e.target.value)}>
                {models?.map((m) => (
                  <option key={m.id} value={m.id}>
                    {m.name}
                  </option>
                ))}
              </select>
            </label>
            <label className="field">
              <span className="small muted">Idioma</span>
              <select value={language} onChange={(e) => setLanguage(e.target.value)}>
                <option value="auto">Detectar automáticamente</option>
                {LANGUAGES.map(([code, name]) => (
                  <option key={code} value={code}>
                    {name}
                  </option>
                ))}
              </select>
            </label>
            <label className="field">
              <span className="small muted">Procesar con</span>
              <select value={device} onChange={(e) => setDevice(e.target.value)}>
                <option value="auto">Automático (GPU si está disponible)</option>
                <option value="cpu">CPU</option>
              </select>
            </label>
            <button
              className="btn primary"
              disabled={!workerReady || !modelId}
              onClick={() => {
                setMode('karaoke')
                props.onStart(modelId || undefined, language, device)
              }}
            >
              <Icon name="mic" size={16} /> Transcribir
            </button>
          </div>
        )}
        {lyrics && (
          <button className="link small" onClick={() => setMode('karaoke')}>
            Volver a la letra
          </button>
        )}
      </div>
    )
  }

  const exportAs = async (f: (typeof EXPORTS)[number]) => {
    try {
      const suffix = f.id === 'lrc_words' ? ' (karaoke)' : ''
      const path = await save({
        defaultPath: `${props.projectName}${suffix}.${f.ext}`,
        filters: [{ name: f.ext.toUpperCase(), extensions: [f.ext] }],
      })
      if (!path) return
      setStatus(`Guardado en ${await api.lyricsExport(path, f.id)}`)
    } catch (e) {
      props.onError(errorMessage(e))
    }
  }

  const lowConfidence = lyrics.language_probability < 0.7
  const header = (
    <div className="panel-row lyrics-head">
      <div className="seg small">
        <button className={mode === 'karaoke' ? 'on' : ''} onClick={() => setMode('karaoke')}>
          Karaoke
        </button>
        <button className={mode === 'edit' ? 'on' : ''} onClick={() => setMode('edit')}>
          Editar
        </button>
      </div>
      <span className="small muted">
        {languageName(lyrics.language)}
        {lowConfidence && ' (poco seguro)'} · {lyrics.lines.length} líneas ·{' '}
        {lyrics.source === 'vocals' ? 'voz aislada' : 'mezcla completa'}
      </span>
      <select
        className="compact"
        value=""
        onChange={(e) => {
          const f = EXPORTS.find((x) => x.id === e.target.value)
          if (f) exportAs(f)
        }}
        aria-label="Exportar letra"
      >
        <option value="">Exportar…</option>
        {EXPORTS.map((f) => (
          <option key={f.id} value={f.id}>
            {f.label}
          </option>
        ))}
      </select>
      <button className="btn ghost small" onClick={() => setMode('setup')}>
        <Icon name="refresh" size={14} /> Volver a transcribir
      </button>
    </div>
  )

  return (
    <div className="panel">
      {header}
      {lowConfidence && (
        <div className="notice small">
          No está claro el idioma ({Math.round(lyrics.language_probability * 100)} %). Si la letra sale mal, vuelve a
          transcribir eligiendo el idioma.
        </div>
      )}
      {lyrics.warning && <div className="notice small">{lyrics.warning}</div>}
      {status && <span className="small muted">{status}</span>}
      {mode === 'karaoke' ? (
        <Karaoke lines={lyrics.lines} position={position} onSeek={props.onSeek} />
      ) : (
        <Editor lyrics={lyrics} position={position} onChange={props.onChange} onSeek={props.onSeek} />
      )}
    </div>
  )
}

function Karaoke({ lines, position, onSeek }: { lines: LyricLine[]; position: number; onSeek: (t: number) => void }) {
  const i = activeLine(lines, position)
  const visible = [i - 1, i, i + 1, i + 2]
  if (lines.length === 0) return <p className="muted">No se encontró letra cantada.</p>
  return (
    <div className="karaoke">
      {visible.map((k, slot) => {
        const line = lines[k]
        if (!line) return <div key={`empty${slot}`} className="karaoke-line empty" />
        const role = k === i ? 'now' : k < i ? 'past' : 'next'
        return (
          <div key={k} className={`karaoke-line ${role}`} onClick={() => onSeek(line.start)}>
            {role === 'now' && line.words.length > 0
              ? line.words.map((w, j) => (
                  <span
                    key={j}
                    className="kw"
                    style={{ ['--p' as string]: `${Math.round(wordProgress(w, position) * 100)}%` }}
                  >
                    {w.text}{' '}
                  </span>
                ))
              : line.text}
          </div>
        )
      })}
      {i < 0 && <span className="small muted">La letra empieza en {formatTime(lines[0].start).slice(0, -3)}</span>}
    </div>
  )
}

function Editor({
  lyrics,
  position,
  onChange,
  onSeek,
}: {
  lyrics: Lyrics
  position: number
  onChange: (l: Lyrics) => void
  onSeek: (t: number) => void
}) {
  const current = activeLine(lyrics.lines, position)
  const setLines = (lines: LyricLine[]) => onChange({ ...lyrics, lines })
  const update = (k: number, line: LyricLine) => setLines(lyrics.lines.map((l, j) => (j === k ? line : l)))

  return (
    <div className="lyrics-editor">
      <div className="panel-row small">
        <span className="muted">Sincronía de toda la letra:</span>
        <button className="btn small" onClick={() => onChange(shiftAll(lyrics, -0.1))}>
          −0,1 s
        </button>
        <button className="btn small" onClick={() => onChange(shiftAll(lyrics, 0.1))}>
          +0,1 s
        </button>
        <button
          className="btn small"
          onClick={() => {
            const lines = [...lyrics.lines, newLineAt(lyrics.lines, position)].sort((a, b) => a.start - b.start)
            setLines(lines)
          }}
        >
          <Icon name="plus" size={14} /> Línea aquí
        </button>
      </div>
      <div className="lyrics-list">
        {lyrics.lines.map((line, k) => (
          <div key={`${k}-${line.start}`} className={`lyrics-row ${k === current ? 'now' : ''}`}>
            <button className="mono small time-btn" title="Ir a esta línea" onClick={() => onSeek(line.start)}>
              {formatTime(line.start).slice(0, -1)}
            </button>
            <button
              className="icon-btn"
              title="La línea empieza en la posición actual"
              onClick={() => update(k, shiftLine(line, position - line.start))}
            >
              <Icon name="pin" size={14} />
            </button>
            <input
              type="text"
              defaultValue={line.text}
              onBlur={(e) => {
                if (e.target.value.trim() !== line.text) update(k, retextLine(line, e.target.value))
              }}
              onKeyDown={(e) => e.key === 'Enter' && (e.target as HTMLInputElement).blur()}
            />
            <button
              className="icon-btn"
              title="Eliminar línea"
              onClick={() => setLines(lyrics.lines.filter((_, j) => j !== k))}
            >
              <Icon name="trash" size={14} />
            </button>
          </div>
        ))}
      </div>
    </div>
  )
}
