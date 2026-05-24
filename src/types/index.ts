export type PrioStatus = 'success' | 'warning' | 'failure'

export type LogLevel = 'info' | 'warn' | 'error'

export interface LogEntry {
  level: LogLevel
  message: string
  timestamp_ms: number
  cli_command?: string
}

export interface PrioResult {
  status: PrioStatus
  message: string
  logs: LogEntry[]
}

export interface RepoRecord {
  id: string
  path: string
  origin_normalized: string
  mc_clone_path: string
  added_at: number
}

export interface BranchInfo {
  name: string
  pr_number?: number
  commits?: CommitInfo[]
}

export interface CommitInfo {
  sha: string
  message: string
  branch: string
}

export interface StatusData {
  applied_branches: BranchInfo[]
  unassigned_commits: CommitInfo[]
}

export interface StatusResult {
  data: StatusData
  prio_result: PrioResult
}

export interface WorkBranchSuggestion {
  default_name: string
  explanation: string
}

export interface RunningOp {
  command: string
  logs: LogEntry[]
}

/** Per-repo UI state for the status panel (keyed by repo id in App). */
export interface RepoPanelState {
  status: StatusResult | null
  lastResult: PrioResult | null
  branchInput: string
  pushBranch: string
  stackDeps: string
  stackBranch: string
  running: boolean
}

export function defaultRepoPanelState(): RepoPanelState {
  return {
    status: null,
    lastResult: null,
    branchInput: '',
    pushBranch: '',
    stackDeps: '',
    stackBranch: '',
    running: false,
  }
}
