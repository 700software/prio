import type { RepoRecord } from '../../types'
import styles from './Header.module.css'

interface Props {
  repos: RepoRecord[]
  tabOrder: string[]
  activeId: string | null
  onSelect: (id: string | null) => void
  onReorder: (order: string[]) => void
  onAddRepo: () => void
}

export function Header({ repos, tabOrder, activeId, onSelect, onReorder, onAddRepo }: Props) {
  const ordered = tabOrder.map(id => repos.find(r => r.id === id)).filter((r): r is RepoRecord => !!r)

  const extras = repos.filter(r => !tabOrder.includes(r.id))
  const tabs = [...ordered, ...extras]

  const onDragStart = (e: React.DragEvent, index: number) => {
    e.dataTransfer.setData('text/plain', String(index))
    e.dataTransfer.effectAllowed = 'move'
  }

  const onDrop = (e: React.DragEvent, dropIndex: number) => {
    e.preventDefault()
    const from = Number(e.dataTransfer.getData('text/plain'))
    if (Number.isNaN(from)) return
    const ids = tabs.map(t => t.id)
    const [moved] = ids.splice(from, 1)
    ids.splice(dropIndex, 0, moved)
    onReorder(ids)
  }

  const label = (r: RepoRecord) => {
    const parts = r.path.replace(/\\/g, '/').split('/')
    return parts[parts.length - 1] || r.path
  }

  return (
    <header className={styles.header}>
      <nav className={styles.tabs}>
        {tabs.map((repo, index) => (
          <button
            key={repo.id}
            type="button"
            draggable
            className={`${styles.tab} ${activeId === repo.id ? styles.active : ''}`}
            onClick={() => onSelect(repo.id)}
            onDragStart={e => onDragStart(e, index)}
            onDragOver={e => e.preventDefault()}
            onDrop={e => onDrop(e, index)}
          >
            {label(repo)}
          </button>
        ))}
        <button type="button" className={styles.addTab} onClick={onAddRepo}>
          + Setup repository
        </button>
      </nav>
    </header>
  )
}
