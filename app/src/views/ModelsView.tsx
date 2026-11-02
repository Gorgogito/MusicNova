import { useCallback, useEffect, useState } from 'react'
import { openUrl } from '@tauri-apps/plugin-opener'
import { api, errorMessage, events, type ModelInfo, type WorkerStatus } from '../lib/api'
import { formatBytes } from '../lib/format'
import { Icon } from '../components/Icon'

const TASK_LABELS: Record<string, string> = {
  separation: 'Separación de stems',
  chords: 'Acordes',
  beats: 'Pulso y tempo',
  key: 'Tonalidad',
  lyrics: 'Letras',
  transcription: 'Audio a MIDI',
  instruments: 'Detección de instrumentos',
  generation: 'Generación',
  tool: 'Herramientas',
}

interface Job {
  progress: number
  message: string
}

export function ModelsView({ worker, onError }: { worker: WorkerStatus | null; onError: (m: string) => void }) {
  const [models, setModels] = useState<ModelInfo[]>([])
  const [manifestErrors, setManifestErrors] = useState<string[]>([])
  // model_id -> trabajo en curso
  const [jobs, setJobs] = useState<Record<string, Job & { jobId: string }>>({})
  const ready = worker?.state === 'ready'

  const refresh = useCallback(() => {
    api
      .worker<{ models: ModelInfo[]; errors: string[] }>('models.list')
      .then((r) => {
        setModels(r.models)
        setManifestErrors(r.errors)
      })
      .catch((e) => onError(errorMessage(e)))
  }, [onError])

  useEffect(() => {
    if (ready) refresh()
  }, [ready, refresh])

  useEffect(() => {
    const un = events.worker((ev) => {
      if (!('job_id' in ev.data) || ev.data.kind !== 'models.install') return
      const jobId = ev.data.job_id
      setJobs((prev) => {
        const entry = Object.entries(prev).find(([, j]) => j.jobId === jobId)
        if (!entry) return prev
        const [modelId] = entry
        if (ev.event === 'job.progress') {
          return { ...prev, [modelId]: { jobId, progress: ev.data.progress, message: ev.data.message } }
        }
        const next = { ...prev }
        delete next[modelId]
        if (ev.event === 'job.failed') onError(`No se pudo instalar ${modelId}: ${ev.data.error}`)
        refresh()
        return next
      })
    })
    return () => {
      un.then((f) => f())
    }
  }, [onError, refresh])

  const install = async (m: ModelInfo) => {
    try {
      const { job_id } = await api.worker<{ job_id: string }>('models.install', { model_id: m.id })
      setJobs((prev) => ({ ...prev, [m.id]: { jobId: job_id, progress: 0, message: 'iniciando…' } }))
    } catch (e) {
      onError(errorMessage(e))
    }
  }

  const cancel = (modelId: string) => {
    const job = jobs[modelId]
    if (job) api.worker('jobs.cancel', { job_id: job.jobId }).catch((e) => onError(errorMessage(e)))
  }

  const remove = async (m: ModelInfo) => {
    try {
      await api.worker('models.remove', { model_id: m.id })
      refresh()
    } catch (e) {
      onError(errorMessage(e))
    }
  }

  const byTask = models.reduce<Record<string, ModelInfo[]>>((acc, m) => {
    ;(acc[m.task] ??= []).push(m)
    return acc
  }, {})

  return (
    <section className="models">
      <header className="view-header">
        <div>
          <h1>Modelos de IA</h1>
          <p className="muted">
            Los modelos se descargan una sola vez, se verifican con SHA-256 y después funcionan sin conexión.
          </p>
        </div>
        <button className="btn ghost" onClick={refresh} disabled={!ready}>
          <Icon name="refresh" size={16} /> Actualizar
        </button>
      </header>

      {!ready && <div className="notice">El proceso de IA no está listo. Revisa el estado en «Sistema».</div>}
      {manifestErrors.map((e) => (
        <div key={e} className="notice error">
          {e}
        </div>
      ))}

      {Object.entries(byTask).map(([task, list]) => (
        <div key={task} className="model-group">
          <h2>{TASK_LABELS[task] ?? task}</h2>
          <div className="model-grid">
            {list.map((m) => {
              const job = jobs[m.id]
              return (
                <article key={m.id} className={`card model ${m.installed ? 'installed' : ''}`}>
                  <div className="model-head">
                    <h3>{m.name}</h3>
                    {m.installed && <span className="badge ok">Instalado</span>}
                  </div>
                  <p className="muted small">{m.description}</p>
                  <div className="badges">
                    <button className="badge link" onClick={() => openUrl(m.license_url)} title="Ver licencia">
                      {m.license}
                    </button>
                    <span className={`badge ${m.commercial_use ? '' : 'warn'}`}>
                      {m.commercial_use ? 'Uso comercial permitido' : 'Solo uso no comercial'}
                    </span>
                    <span className="badge">{formatBytes(m.size_bytes)}</span>
                    {m.min_vram_gb != null && <span className="badge">GPU ≥ {m.min_vram_gb} GB (opcional)</span>}
                  </div>
                  {m.stems && <p className="small">Stems: {m.stems.join(', ')}</p>}
                  {job ? (
                    <div className="job">
                      <div className="progress">
                        <div style={{ width: `${job.progress * 100}%` }} />
                      </div>
                      <span className="small muted">
                        {Math.round(job.progress * 100)}% · {job.message}
                      </span>
                      <button className="btn ghost small" onClick={() => cancel(m.id)}>
                        <Icon name="x" size={14} /> Cancelar
                      </button>
                    </div>
                  ) : m.installed ? (
                    <button className="btn ghost" onClick={() => remove(m)}>
                      <Icon name="trash" size={16} /> Eliminar
                    </button>
                  ) : (
                    <button className="btn primary" onClick={() => install(m)} disabled={!ready}>
                      <Icon name="download" size={16} /> Descargar
                    </button>
                  )}
                </article>
              )
            })}
          </div>
        </div>
      ))}
    </section>
  )
}
