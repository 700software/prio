import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { CommandBadge } from '../CommandBadge/CommandBadge'
import { LogViewer } from '../LogViewer/LogViewer'
import type { PrioResult, WorkBranchSuggestion } from '../../types'
import styles from './SetupPanel.module.css'

interface Props {
  onComplete: () => void
}

export function SetupPanel({ onComplete }: Props) {
  const [repoPath, setRepoPath] = useState('')
  const [mcPath, setMcPath] = useState('')
  const [workBranch, setWorkBranch] = useState('')
  const [suggestion, setSuggestion] = useState<WorkBranchSuggestion | null>(null)
  const [running, setRunning] = useState(false)
  const [result, setResult] = useState<PrioResult | null>(null)

  useEffect(() => {
    if (!repoPath.trim()) {
      setSuggestion(null)
      return
    }
    void invoke<WorkBranchSuggestion>('prio_suggest_work_branch', {
      repoPath: repoPath.trim(),
    })
      .then(setSuggestion)
      .catch(() => setSuggestion(null))
  }, [repoPath])

  useEffect(() => {
    if (!repoPath.trim()) return
    const name = repoPath.replace(/\\/g, '/').split('/').pop() || 'repo'
    const parent = repoPath.replace(/\\/g, '/').split('/').slice(0, -1).join('/')
    setMcPath(`${parent}/${name}-prio-mc`)
  }, [repoPath])

  const cliCmd = repoPath.trim()
    ? `prio setup ${repoPath.trim()}${mcPath ? ` ${mcPath}` : ''}`
    : `prio setup${mcPath ? ` ${mcPath}` : ''}`

  const submit = async () => {
    setRunning(true)
    setResult(null)
    try {
      const res = await invoke<PrioResult>('prio_setup', {
        repoPath: repoPath.trim(),
        mcPath: mcPath.trim() || null,
        workBranch: workBranch.trim() || null,
      })
      setResult(res)
      if (res.status !== 'failure') onComplete()
    } catch (e) {
      setResult({
        status: 'failure',
        message: String(e),
        logs: [],
      })
    } finally {
      setRunning(false)
    }
  }

  return (
    <section className={styles.panel}>
      <h2>Set up repository</h2>
      <CommandBadge command={cliCmd} />
      <label className={styles.label}>
        Repository path
        <input value={repoPath} onChange={e => setRepoPath(e.target.value)} placeholder="C:\GitHub\my-repo" />
      </label>
      <label className={styles.label}>
        Merge-conflicts clone (prio-mc)
        <input value={mcPath} onChange={e => setMcPath(e.target.value)} placeholder="…-prio-mc" />
      </label>
      <label className={styles.label}>
        Work branch
        <input
          value={workBranch}
          onChange={e => setWorkBranch(e.target.value)}
          placeholder={suggestion?.default_name ?? 'prio/yourname'}
        />
      </label>
      {suggestion && <p className={styles.hint}>{suggestion.explanation}</p>}
      <button
        type="button"
        className={styles.primary}
        disabled={!repoPath.trim() || running}
        onClick={() => void submit()}
      >
        Set up repository
      </button>
      <LogViewer result={result} isRunning={running} defaultOpen />
    </section>
  )
}
