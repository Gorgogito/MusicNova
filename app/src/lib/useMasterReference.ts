import { useCallback, useEffect, useRef, useState } from 'react'
import { api, errorMessage, events, type MasterReferenceResult, type ProjectView } from './api'
import { tr } from '../i18n'

export interface MasterJob {
  jobId: string | null
  progress: number
  message: string
}

/** Sigue un trabajo de master por referencia; al terminar añade el master como pista. */
export function useMasterReference(
  onApplied: (project: ProjectView, result: MasterReferenceResult) => void,
  onError: (m: string) => void,
) {
  const [job, setJob] = useState<MasterJob | null>(null)
  const jobRef = useRef<MasterJob | null>(null)
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
          const result = ev.data.result as MasterReferenceResult
          setJob({ ...current, progress: 1, message: tr('cargando el master…') })
          api
            .masterReferenceApply(result.output, result.reference)
            .then((p) => onApplied(p, result))
            .catch((e) => onError(errorMessage(e)))
            .finally(() => setJob(null))
          break
        }
        case 'job.failed':
          onError(tr('La masterización falló: {error}', { error: ev.data.error }))
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
  }, [onApplied, onError])

  const start = useCallback(
    async (reference: string) => {
      setJob({ jobId: null, progress: 0, message: tr('preparando la mezcla…') })
      try {
        const { job_id } = await api.masterReferenceStart(reference)
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

  return { job, start, cancel }
}
