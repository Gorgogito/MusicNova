import { useCallback, useEffect, useRef, useState } from 'react'
import { api, errorMessage, events, type ProjectView, type SeparationResult } from './api'
import { tr } from '../i18n'

export interface SeparationJob {
  jobId: string | null
  modelId: string
  progress: number
  message: string
}

/**
 * Sigue un trabajo de separación de principio a fin. Vive en App para que el
 * trabajo no se pierda al cambiar de pantalla.
 */
export function useSeparation(onProject: (p: ProjectView) => void, onError: (m: string) => void) {
  const [job, setJob] = useState<SeparationJob | null>(null)
  const [lastResult, setLastResult] = useState<SeparationResult | null>(null)
  const jobRef = useRef<SeparationJob | null>(null)
  useEffect(() => {
    jobRef.current = job
  }, [job])

  useEffect(() => {
    const un = events.worker((ev) => {
      const current = jobRef.current
      if (!current?.jobId || !('job_id' in ev.data) || ev.data.job_id !== current.jobId) return
      switch (ev.event) {
        case 'job.progress':
          setJob({ ...current, progress: ev.data.progress, message: ev.data.message })
          break
        case 'job.done': {
          const result = ev.data.result as SeparationResult
          setJob({ ...current, progress: 1, message: tr('cargando stems…') })
          api
            .separateApply(result.stems)
            .then((p) => {
              setLastResult(result)
              onProject(p)
            })
            .catch((e) => onError(errorMessage(e)))
            .finally(() => setJob(null))
          break
        }
        case 'job.failed':
          onError(tr('La separación falló: {error}', { error: ev.data.error }))
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
  }, [onProject, onError])

  const start = useCallback(
    async (modelId: string, device: string) => {
      setJob({ jobId: null, modelId, progress: 0, message: tr('preparando audio…') })
      try {
        const { job_id } = await api.separateStart(modelId, device)
        setJob((j) => j && { ...j, jobId: job_id })
      } catch (e) {
        setJob(null)
        onError(errorMessage(e))
      }
    },
    [onError],
  )

  const cancel = useCallback(() => {
    const id = jobRef.current?.jobId
    if (id) api.worker('jobs.cancel', { job_id: id }).catch((e) => onError(errorMessage(e)))
  }, [onError])

  return { job, lastResult, start, cancel }
}
