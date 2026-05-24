import { useCallback, useEffect, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import type { Stuple } from 'stuple'
import { CommitList } from '../CommitList/CommitList'
import { LogViewer } from '../LogViewer/LogViewer'
import type { BranchInfo, CommitInfo, PrioResult, RepoPanelState, StatusResult } from '../../types'
import styles from './StatusPanel.module.css'

interface Props {
  repoPath: string
  panel: Stuple<RepoPanelState>
  onUnsetupComplete: () => void
}

function GearIcon() {
  return (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden>
      <circle cx="12" cy="12" r="3" />
      <path d="M12 1v2M12 21v2M4.22 4.22l1.42 1.42M18.36 18.36l1.42 1.42M1 12h2M21 12h2M4.22 19.78l1.42-1.42M18.36 5.64l1.42-1.42" />
    </svg>
  )
}

export function StatusPanel({ repoPath, panel, onUnsetupComplete }: Props) {
  const { val: p, set: setPanel } = panel
  const [menuOpen, setMenuOpen] = useState(false)
  const settingsRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!menuOpen) return
    const onPointerDown = (e: MouseEvent) => {
      if (settingsRef.current && !settingsRef.current.contains(e.target as Node)) {
        setMenuOpen(false)
      }
    }
    document.addEventListener('mousedown', onPointerDown)
    return () => document.removeEventListener('mousedown', onPointerDown)
  }, [menuOpen])

  const load = useCallback(async () => {
    try {
      const s = await invoke<StatusResult>('prio_status', { repoPath })
      setPanel(prev => ({ ...prev, status: s, lastResult: s.prio_result }))
    } catch (e) {
      setPanel(prev => ({
        ...prev,
        lastResult: {
          status: 'failure',
          message: String(e),
          logs: [],
        },
      }))
    }
  }, [repoPath, setPanel])

  useEffect(() => {
    void load()
  }, [load])

  const run = async (_command: string, fn: () => Promise<PrioResult>) => {
    setPanel(prev => ({ ...prev, running: true }))
    try {
      const res = await fn()
      setPanel(prev => ({ ...prev, lastResult: res }))
      await load()
      return res
    } catch (e) {
      setPanel(prev => ({
        ...prev,
        lastResult: {
          status: 'failure',
          message: String(e),
          logs: [],
        },
      }))
    } finally {
      setPanel(prev => ({ ...prev, running: false }))
    }
  }

  const apply = () =>
    run(`prio apply ${p.branchInput}`, () =>
      invoke('prio_apply', {
        repoPath,
        branches: p.branchInput.split(/\s+/).filter(Boolean),
      }),
    )

  const unapply = () =>
    run(`prio unapply ${p.branchInput}`, () =>
      invoke('prio_unapply', {
        repoPath,
        branches: p.branchInput.split(/\s+/).filter(Boolean),
      }),
    )

  const push = () => run(`prio push ${p.pushBranch}`, () => invoke('prio_push', { repoPath, branch: p.pushBranch }))

  const pr = () => run(`prio pr ${p.pushBranch}`, () => invoke('prio_pr', { repoPath, branch: p.pushBranch }))

  const sync = () => run('prio sync', () => invoke('prio_sync', { repoPath }))

  const recover = () => run('prio recover', () => invoke('prio_recover', { repoPath }))

  const stack = () =>
    run(`prio stack ${p.stackDeps} ${p.stackBranch}`, () =>
      invoke('prio_stack', { repoPath, dependencies: p.stackDeps, branch: p.stackBranch }),
    )

  const handleUnsetup = async () => {
    setMenuOpen(false)
    const confirmed = window.confirm(
      `Remove prio setup for this repository?\n\n${repoPath}\n\n` +
        'This archives .git/prio, renames the work branch, backs up the merge-conflicts clone, ' +
        'and removes the repo from your prio configuration.',
    )
    if (!confirmed) return

    setPanel(prev => ({ ...prev, running: true }))
    try {
      const res = await invoke<PrioResult>('prio_unsetup', { repoPath })
      setPanel(prev => ({ ...prev, lastResult: res }))
      if (res.status !== 'failure') {
        onUnsetupComplete()
      }
    } catch (e) {
      setPanel(prev => ({
        ...prev,
        lastResult: {
          status: 'failure',
          message: String(e),
          logs: [],
        },
      }))
    } finally {
      setPanel(prev => ({ ...prev, running: false }))
    }
  }

  const onCommitDrop = (sha: string, destBranch: string) => {
    void run(`prio mv ${sha} ${destBranch}`, () =>
      invoke('prio_mv', {
        repoPath,
        commits: [sha],
        destination: destBranch,
        create: false,
      }),
    )
  }

  const branches: BranchInfo[] = p.status?.data.applied_branches ?? []
  const unassigned: CommitInfo[] = p.status?.data.unassigned_commits ?? []

  return (
    <section className={styles.panel}>
      <div className={styles.headerRow}>
        <h2>
          Work area status for {repoPath}{' '}
          <span className={styles.cliHint}>
            (<code>prio status</code>)
          </span>
        </h2>
        <div className={styles.settingsWrap} ref={settingsRef}>
          <button
            type="button"
            className={styles.gearBtn}
            aria-label="Repository settings"
            aria-expanded={menuOpen}
            aria-haspopup="menu"
            disabled={p.running}
            onClick={() => setMenuOpen(open => !open)}
          >
            <GearIcon />
          </button>
          {menuOpen && (
            <div className={styles.menu} role="menu">
              <button
                type="button"
                role="menuitem"
                className={`${styles.menuItem} ${styles.menuItemDanger}`}
                onClick={() => void handleUnsetup()}
              >
                Unsetup
              </button>
            </div>
          )}
        </div>
      </div>

      <div className={styles.columns}>
        <div className={styles.branchCol}>
          <h3>Applied branches</h3>
          <ul>
            {branches.map(b => (
              <li key={b.name}>
                <div>
                  {b.name}
                  {b.pr_number != null && <span> (PR #{b.pr_number})</span>}
                </div>
                {(b.commits?.length ?? 0) > 0 && (
                  <ul className={styles.branchCommits}>
                    {b.commits!.map(c => (
                      <li key={c.sha}>
                        <code>{c.sha.slice(0, 7)}</code> {c.message}
                      </li>
                    ))}
                  </ul>
                )}
              </li>
            ))}
            {branches.length === 0 && <li className={styles.muted}>(none)</li>}
          </ul>
        </div>
        <CommitList title="Unassigned commits" commits={unassigned} onDragStart={() => {}} />
      </div>

      <p className={styles.muted}>
        Drag a commit onto a branch name below to run <code>prio mv</code>
      </p>
      <div className={styles.dropTargets}>
        {branches.map(b => (
          <CommitList
            key={b.name}
            title={`Drop → ${b.name}`}
            commits={[]}
            droppable
            onDrop={sha => onCommitDrop(sha, b.name)}
          />
        ))}
        <CommitList title="Drop → unassigned (.)" commits={[]} droppable onDrop={sha => onCommitDrop(sha, '.')} />
      </div>

      <div className={styles.actions}>
        <input
          placeholder="branch or pr-123"
          value={p.branchInput}
          onChange={e => setPanel(prev => ({ ...prev, branchInput: e.target.value }))}
        />
        <button type="button" onClick={() => void apply()} disabled={p.running}>
          Apply
        </button>
        <button type="button" onClick={() => void unapply()} disabled={p.running}>
          Unapply
        </button>
      </div>

      <div className={styles.actions}>
        <input
          placeholder="branch to push/pr"
          value={p.pushBranch}
          onChange={e => setPanel(prev => ({ ...prev, pushBranch: e.target.value }))}
        />
        <button type="button" onClick={() => void push()} disabled={p.running}>
          Push
        </button>
        <button type="button" onClick={() => void pr()} disabled={p.running}>
          PR
        </button>
      </div>

      <div className={styles.actions}>
        <input
          placeholder="deps (a+b)"
          value={p.stackDeps}
          onChange={e => setPanel(prev => ({ ...prev, stackDeps: e.target.value }))}
        />
        <input
          placeholder="stacked branch"
          value={p.stackBranch}
          onChange={e => setPanel(prev => ({ ...prev, stackBranch: e.target.value }))}
        />
        <button type="button" onClick={() => void stack()} disabled={p.running}>
          Stack
        </button>
      </div>

      <div className={styles.actions}>
        <button type="button" onClick={() => void sync()} disabled={p.running}>
          Sync
        </button>
        <button type="button" onClick={() => void recover()} disabled={p.running}>
          Recover
        </button>
        <button type="button" onClick={() => void load()} disabled={p.running}>
          Refresh
        </button>
      </div>

      <LogViewer key={repoPath} result={p.lastResult} isRunning={p.running} />
    </section>
  )
}
