import { useCallback, useEffect, useRef, useState } from 'react'
import { api, errorMessage, events, type Analysis, type PracticeSettings } from './api'

export interface AnalysisJob {
  jobId: string | null
  progress: number
  message: string
}

/**
 * Sigue un trabajo de análisis musical. Vive en App para que no se pierda al
 * cambiar de pantalla; al terminar entrega el análisis y los ajustes de
 * práctica alineados con él (BPM, compás, metrónomo sobre los pulsos).
 */
export function useAnalysis(
  onApplied: (analysis: Analysis, practice: PracticeSettings) => void,
  onError: (m: string) => void,
) {
  const [job, setJob] = useState<AnalysisJob | null>(null)
  const jobRef = useRef<AnalysisJob | null>(null)
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
        case 'job.done':
          api
            .analysisApply()
            .then((r) => onApplied(r.analysis, r.practice))
            .catch((e) => onError(errorMessage(e)))
            .finally(() => setJob(null))
          break
        case 'job.failed':
          onError(`El análisis falló: ${ev.data.error}`)
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
    async (device: string, beatModelId?: string) => {
      setJob({ jobId: null, progress: 0, message: 'preparando audio…' })
      try {
        const { job_id } = await api.analyzeStart(device, beatModelId)
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
