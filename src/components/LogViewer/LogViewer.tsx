import { useState } from 'react'
import type { LogEntry, PrioResult, PrioStatus } from '../../types'
import styles from './LogViewer.module.css'

interface Props {
  result?: PrioResult | null
  isRunning?: boolean
  defaultOpen?: boolean
}

export function LogViewer({ result, isRunning = false, defaultOpen }: Props) {
  const [open, setOpen] = useState(defaultOpen ?? isRunning)

  if (!result && !isRunning) return null

  const status: PrioStatus | 'running' = isRunning ? 'running' : result!.status
  const message = isRunning ? 'Running…' : result!.message
  const logs: LogEntry[] = result?.logs ?? []

  return (
    <div className={styles.wrap}>
      <div className={`${styles.conclusion} ${styles[status]}`}>{message}</div>
      {(logs.length > 0 || isRunning) && (
        <details open={open} onToggle={e => setOpen((e.target as HTMLDetailsElement).open)}>
          <summary className={styles.summary}>Log output ({logs.length})</summary>
          <ul className={styles.list}>
            {logs.map((entry, i) => (
              <li key={`${entry.timestamp_ms}-${i}`} className={styles[entry.level]}>
                <span className={styles.level}>[{entry.level}]</span>
                <span>{entry.message}</span>
                {entry.cli_command && <code className={styles.cmd}>{entry.cli_command}</code>}
              </li>
            ))}
          </ul>
        </details>
      )}
    </div>
  )
}
