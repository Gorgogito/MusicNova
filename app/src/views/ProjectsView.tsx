import { useEffect, useState } from 'react'
import { open } from '@tauri-apps/plugin-dialog'
import { api, errorMessage, type AppInfo, type ProjectSummary } from '../lib/api'
import { formatDate } from '../lib/format'
import { Icon } from '../components/Icon'

interface Props {
  info: AppInfo | null
  busy: string | null
  onImport: (path: string) => void
  onOpen: (dir: string) => void
  onError: (msg: string) => void
}

export function ProjectsView({ info, busy, onImport, onOpen, onError }: Props) {
  const [projects, setProjects] = useState<ProjectSummary[]>([])

  useEffect(() => {
    api.listProjects().then(setProjects).catch((e) => onError(errorMessage(e)))
  }, [busy, onError])

  const pickFile = async () => {
    const path = await open({
      multiple: false,
      directory: false,
      filters: [{ name: 'Audio', extensions: info?.supported_extensions ?? ['wav', 'mp3', 'flac'] }],
    })
    if (typeof path === 'string') onImport(path)
  }

  return (
    <section className="projects">
      <header className="view-header">
        <div>
          <h1>Proyectos</h1>
          <p className="muted">Importa una canción para empezar. Todo se procesa en tu equipo.</p>
        </div>
      </header>

      <button className="dropzone" onClick={pickFile} disabled={!!busy}>
        <Icon name="music" size={36} />
        <strong>{busy ?? 'Importar audio o vídeo'}</strong>
        <span className="muted">
          Haz clic o arrastra un archivo aquí ·{' '}
          {(info?.supported_extensions ?? []).filter((e) => !['wave', 'oga'].includes(e)).join(', ').toUpperCase()}
        </span>
      </button>

      {info && !info.ffmpeg_available && (
        <p className="small muted">
          Para importar vídeo (MOV, AVI…) y formatos como Opus o WMA, instala FFmpeg desde «Modelos IA» →
          Herramientas.
        </p>
      )}

      <h2>Recientes</h2>
      {projects.length === 0 ? (
        <p className="muted">Aún no hay proyectos en {info?.projects_root ?? 'la carpeta de música'}.</p>
      ) : (
        <ul className="project-list">
          {projects.map((p) => (
            <li key={p.dir}>
              <button className="project-item" onClick={() => onOpen(p.dir)} disabled={!!busy}>
                <Icon name="folder" />
                <span className="project-name">{p.name}</span>
                <span className="muted small">
                  {p.tracks} {p.tracks === 1 ? 'pista' : 'pistas'}
                </span>
                <span className="muted small">{formatDate(p.modified)}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  )
}
