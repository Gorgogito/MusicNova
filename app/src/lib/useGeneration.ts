import { useCallback, useEffect, useRef, useState } from 'react'
import { api, errorMessage, events, type GeneratedTrack, type ProjectView } from './api'

export type GenerationKind = 'voice' | 'generate' | 'install'

export interface GenerationJob {
  kind: GenerationKind
  /** Pista de origen (voz) o pista que se genera. */
  target: string | null
  jobId: string | null
  progress: number
  message: string
  startedAt: number
}

export const GENERATION_LABEL: Record<GenerationKind, string> = {
  voice: 'Limpiando la voz',
  generate: 'Generando acompañamiento',
  install: 'Instalando ACE-Step',
}

/** Sigue el trabajo de IA generativa en curso (uno a la vez; vive en App). */
export function useGeneration(onProject: (p: ProjectView) => void, onInstalled: () => void, onError: (m: string) => void) {
  const [job, setJob] = useState<GenerationJob | null>(null)
  const jobRef = useRef<GenerationJob | null>(null)
  const cb = useRef({ onProject, onInstalled })
  useEffect(() => {
    jobRef.current = job
    cb.current = { onProject, onInstalled }
  })

  useEffect(() => {
    const un = events.worker((ev) => {
      const current = jobRef.current
      if (!current?.jobId || !('job_id' in ev.data) || ev.data.job_id !== current.jobId) return
      switch (ev.event) {
        case 'job.progress':
          setJob({ ...current, progress: ev.data.progress, message: ev.data.message })
          break
        case 'job.done': {
          const apply =
            current.kind === 'voice'
              ? api.denoiseVoiceApply(current.target!).then(cb.current.onProject)
              : current.kind === 'generate'
                ? api.generateApply(current.target as GeneratedTrack).then(cb.current.onProject)
                : Promise.resolve(cb.current.onInstalled())
          apply.catch((e) => onError(errorMessage(e))).finally(() => setJob(null))
          break
        }
        case 'job.failed':
          onError(`${GENERATION_LABEL[current.kind]}: falló (${ev.data.error})`)
          setJob(null)
          break
        case 'job.cancelled':
          setJob(null)
          break
      }
    })
    return () => {
      un.then((f) => f())
    }
  }, [onError])

  const run = useCallback(
    async (kind: GenerationKind, target: string | null, start: () => Promise<{ job_id: string }>) => {
      if (jobRef.current) {
        onError('Ya hay un trabajo de IA generativa en curso')
        return
      }
      const pending = { kind, target, jobId: null, progress: 0, message: 'preparando audio…', startedAt: Date.now() }
      jobRef.current = pending
      setJob(pending)
      try {
        const { job_id } = await start()
        setJob((j) => j && { ...j, jobId: job_id })
      } catch (e) {
        jobRef.current = null
        setJob(null)
        onError(errorMessage(e))
      }
    },
    [onError],
  )

  const cleanVoice = useCallback(
    (trackId: string, attenLimDb: number | null) => run('voice', trackId, () => api.denoiseVoiceStart(trackId, attenLimDb)),
    [run],
  )
  const generate = useCallback(
    (track: GeneratedTrack, caption: string, context: 'audible' | 'original', steps: number, seed: number | null) =>
      run('generate', track, () => api.generateStart(track, caption, context, steps, seed)),
    [run],
  )
  const installAceStep = useCallback(
    () => run('install', null, () => api.worker<{ job_id: string }>('tasks.run', { task: 'install_acestep', params: {} })),
    [run],
  )
  const cancel = useCallback(() => {
    const id = jobRef.current?.jobId
    if (id) api.worker('jobs.cancel', { job_id: id }).catch((e) => onError(errorMessage(e)))
  }, [onError])

  return { job, cleanVoice, generate, installAceStep, cancel }
}

export type GenerationJobs = ReturnType<typeof useGeneration>
