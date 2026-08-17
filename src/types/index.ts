export type PrioStatus = "success" | "warning" | "failure";

export type LogLevel = "info" | "warn" | "error";

export interface CliCommandLog {
  cwd: string;
  command: string;
  comment?: string;
}

export interface LogEntry {
  level: LogLevel;
  message: string;
  timestamp_ms: number;
  cli?: CliCommandLog;
}

export interface PrioResult {
  status: PrioStatus;
  message: string;
  logs: LogEntry[];
}

export interface RepoRecord {
  id: string;
  path: string;
  origin_normalized: string;
  mc_clone_path: string;
  added_at: number;
}

export interface BranchInfo {
  name: string;
  pr_number?: number;
  pr_url?: string;
  commits?: CommitInfo[];
  /** "merged" | "conflict" | "pending" during an in-progress apply */
  apply_status?: string;
  applied: boolean;
  /** Branches this branch is stacked after (from `prio stack`), if any. */
  stacked_after?: string[];
}

export interface CommitInfo {
  sha: string;
  message: string;
  branch: string;
  /** True when the commit is actually on the branch's git ref (origin/<branch>).
   *  False when it exists only above the work-area baseline — assigned via
   *  commit_assignments but not yet merged into the branch. */
  on_branch: boolean;
}

export interface MergeConflictInfo {
  mc_path: string;
  merge_branch: string;
  incoming_branch: string;
  base_desc: string;
  branches_merged: string[];
  branches_pending: string[];
}

export interface MvRebaseConflict {
  source_branch: string;
  dest_branch: string;
  conflicting_commit: string;
  source_is_pushed: boolean;
  mc_path: string;
}

export interface StatusData {
  applied_branches: BranchInfo[];
  unassigned_commits: CommitInfo[];
  merge_conflict?: MergeConflictInfo;
  mv_rebase_conflict?: MvRebaseConflict;
}

export interface StatusResult {
  data: StatusData;
  prio_result: PrioResult;
}

export interface WorkBranchSuggestion {
  default_name: string;
  explanation: string;
}

export interface RunningOp {
  command: string;
  logs: LogEntry[];
}

/** Per-repo UI state for the status panel (keyed by repo id in App). */
export interface RepoPanelState {
  status: StatusResult | null;
  lastResult: PrioResult | null;
  branchInput: string;
  pushBranch: string;
  stackDeps: string;
  stackBranch: string;
  running: boolean;
}

export function defaultRepoPanelState(): RepoPanelState {
  return {
    status: null,
    lastResult: null,
    branchInput: "",
    pushBranch: "",
    stackDeps: "",
    stackBranch: "",
    running: false,
  };
}
