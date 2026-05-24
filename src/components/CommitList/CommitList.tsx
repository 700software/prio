import type { CommitInfo } from '../../types'
import styles from './CommitList.module.css'

interface Props {
  title: string
  commits: CommitInfo[]
  onDragStart?: (sha: string) => void
  droppable?: boolean
  onDrop?: (sha: string) => void
}

export function CommitList({ title, commits, onDragStart, droppable, onDrop }: Props) {
  return (
    <div
      className={`${styles.column} ${droppable ? styles.droppable : ''}`}
      onDragOver={e => droppable && e.preventDefault()}
      onDrop={e => {
        e.preventDefault()
        const sha = e.dataTransfer.getData('text/sha')
        if (sha && onDrop) onDrop(sha)
      }}
    >
      <h3>{title}</h3>
      <ul>
        {commits.length === 0 && <li className={styles.empty}>(none)</li>}
        {commits.map(c => (
          <li
            key={c.sha}
            draggable={!!onDragStart}
            onDragStart={e => {
              e.dataTransfer.setData('text/sha', c.sha)
              onDragStart?.(c.sha)
            }}
          >
            <code>{c.sha.slice(0, 7)}</code>
            <span>{c.message}</span>
          </li>
        ))}
      </ul>
    </div>
  )
}
