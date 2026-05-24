import { useCallback, useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useStuple, useSubStuple, type Stuple } from 'stuple'
import { Header } from './components/Header/Header'
import { SetupPanel } from './components/SetupPanel/SetupPanel'
import { StatusPanel } from './components/StatusPanel/StatusPanel'
import type { RepoPanelState, RepoRecord } from './types'
import { defaultRepoPanelState } from './types'
import styles from './App.module.css'

const EMPTY_REPO_PANEL = defaultRepoPanelState()

function ActiveRepoPanel({
  repo,
  panelByRepo,
  onUnsetupComplete,
}: {
  repo: RepoRecord
  panelByRepo: Stuple<Record<string, RepoPanelState>>
  onUnsetupComplete: () => void
}) {
  const panel = useSubStuple(panelByRepo, repo.id, EMPTY_REPO_PANEL)
  return <StatusPanel repoPath={repo.path} panel={panel} onUnsetupComplete={onUnsetupComplete} />
}

function App() {
  const [repos, setRepos] = useState<RepoRecord[]>([])
  const [tabOrder, setTabOrder] = useState<string[]>([])
  const [activeId, setActiveId] = useState<string | null>(null)
  const [showSetup, setShowSetup] = useState(false)
  const panelByRepo = useStuple<Record<string, RepoPanelState>>(() => ({}))

  const refresh = useCallback(async () => {
    const list = await invoke<RepoRecord[]>('prio_list_repos')
    setRepos(list)
    const ui = await invoke<{ tab_order: string[] }>('prio_load_ui_state')
    const order = ui.tab_order.length ? ui.tab_order.filter(id => list.some(r => r.id === id)) : list.map(r => r.id)
    setTabOrder(order)
    if (!showSetup && order.length > 0 && !activeId) {
      setActiveId(order[0])
    }
    if (order.length === 0) {
      setShowSetup(true)
      setActiveId(null)
    }
  }, [activeId, showSetup])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const onReorder = async (order: string[]) => {
    setTabOrder(order)
    await invoke('prio_save_ui_state', { tabOrder: order })
  }

  const handleUnsetupComplete = useCallback(
    async (removedRepoId: string) => {
      panelByRepo.set(prev => {
        const next = { ...prev }
        delete next[removedRepoId]
        return next
      })

      const list = await invoke<RepoRecord[]>('prio_list_repos')
      setRepos(list)

      const order = tabOrder.filter(id => id !== removedRepoId && list.some(r => r.id === id))
      setTabOrder(order)
      await invoke('prio_save_ui_state', { tabOrder: order })

      if (activeId === removedRepoId) {
        const nextId = order[0] ?? null
        setActiveId(nextId)
        if (order.length === 0) {
          setShowSetup(true)
          setActiveId(null)
        }
      }
    },
    [activeId, panelByRepo, tabOrder],
  )

  const activeRepo = repos.find(r => r.id === activeId)

  return (
    <div className={styles.app}>
      <Header
        repos={repos}
        tabOrder={tabOrder}
        activeId={activeId}
        onSelect={id => {
          setActiveId(id)
          setShowSetup(false)
        }}
        onReorder={order => void onReorder(order)}
        onAddRepo={() => {
          setShowSetup(true)
          setActiveId(null)
        }}
      />
      <main className={styles.main}>
        {showSetup || !activeRepo ? (
          <SetupPanel
            onComplete={() => {
              setShowSetup(false)
              void refresh().then(() => {
                invoke<RepoRecord[]>('prio_list_repos').then(list => {
                  if (list.length > 0) setActiveId(list[list.length - 1].id)
                })
              })
            }}
          />
        ) : (
          <ActiveRepoPanel
            key={activeRepo.id}
            repo={activeRepo}
            panelByRepo={panelByRepo}
            onUnsetupComplete={() => void handleUnsetupComplete(activeRepo.id)}
          />
        )}
      </main>
    </div>
  )
}

export default App
