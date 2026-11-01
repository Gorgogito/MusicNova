import { useEffect, useState } from 'react'
import { api, errorMessage, type AppInfo, type SystemInfo, type WorkerStatus } from '../lib/api'
import { formatBytes } from '../lib/format'
import { Icon } from '../components/Icon'

const STATE_LABEL: Record<WorkerStatus['state'], string> = {
  stopped: 'Detenido',
  starting: 'Iniciando…',
  ready: 'Listo',
  crashed: 'Con errores',
}

export function SystemView({ info, worker, onError }: { info: AppInfo | null; worker: WorkerStatus | null; onError: (m: string) => void }) {
  const [sys, setSys] = useState<SystemInfo | null>(null)

  useEffect(() => {
    if (worker?.state !== 'ready') return
    api.worker<SystemInfo>('system.info').then(setSys).catch((e) => onError(errorMessage(e)))
  }, [worker?.state, onError])

  const accel = sys?.torch?.cuda
    ? `CUDA ${sys.torch.cuda_version ?? ''}`
    : sys?.torch
      ? 'PyTorch en CPU'
      : 'PyTorch no instalado (se instalará con el primer modelo que lo necesite)'

  return (
    <section className="system">
      <header className="view-header">
        <div>
          <h1>Sistema</h1>
          <p className="muted">MusicNova {info?.version} · GPL-3.0</p>
        </div>
      </header>

      <div className="card-grid">
        <article className="card">
          <h3>Audio</h3>
          {info?.engine_error ? (
            <p className="error-text">{info.engine_error}</p>
          ) : (
            <dl>
              <dt>Dispositivo</dt>
              <dd>{info?.device ?? '—'}</dd>
              <dt>Frecuencia</dt>
              <dd>{info?.sample_rate ? `${info.sample_rate} Hz` : '—'}</dd>
              <dt>Proyectos</dt>
              <dd className="mono small">{info?.projects_root}</dd>
            </dl>
          )}
        </article>

        <article className="card">
          <div className="model-head">
            <h3>Proceso de IA</h3>
            <span className={`badge ${worker?.state === 'ready' ? 'ok' : worker?.state === 'crashed' ? 'warn' : ''}`}>
              {worker ? STATE_LABEL[worker.state] : '—'}
            </span>
          </div>
          <dl>
            <dt>Python</dt>
            <dd className="mono small">{sys?.python ? `${sys.python} · ${worker?.python}` : worker?.python}</dd>
            <dt>Datos</dt>
            <dd className="mono small">{sys?.data_dir ?? '—'}</dd>
          </dl>
          {worker?.error && <p className="error-text">{worker.error}</p>}
          {worker?.state === 'crashed' && worker.stderr_tail.length > 0 && (
            <pre className="log">{worker.stderr_tail.join('\n')}</pre>
          )}
          <button className="btn ghost" onClick={() => api.workerRestart().catch((e) => onError(errorMessage(e)))}>
            <Icon name="refresh" size={16} /> Reiniciar
          </button>
        </article>

        <article className="card">
          <h3>Hardware</h3>
          <dl>
            <dt>CPU</dt>
            <dd>{sys ? `${sys.cpu_count} hilos · ${sys.machine}` : '—'}</dd>
            <dt>RAM</dt>
            <dd>{formatBytes(sys?.ram_bytes)}</dd>
            <dt>GPU</dt>
            <dd>
              {sys?.gpus.length
                ? sys.gpus.map((g) => `${g.name} (${formatBytes(g.vram_bytes)})`).join(', ')
                : 'No se detectó GPU NVIDIA'}
            </dd>
            <dt>Aceleración</dt>
            <dd>{sys ? accel : '—'}</dd>
            <dt>Sistema</dt>
            <dd className="small">{sys?.platform ?? '—'}</dd>
          </dl>
        </article>
      </div>
    </section>
  )
}
