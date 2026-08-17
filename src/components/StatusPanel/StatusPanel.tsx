import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Stuple } from "stuple";
import { CommitList } from "../CommitList/CommitList";
import { LogViewer } from "../LogViewer/LogViewer";
import type {
  BranchInfo,
  CommitInfo,
  PrioResult,
  RepoPanelState,
  StatusResult,
} from "../../types";
import styles from "./StatusPanel.module.css";

interface Props {
  repoPath: string;
  panel: Stuple<RepoPanelState>;
  onUnsetupComplete: () => void;
}

export function StatusPanel({ repoPath, panel, onUnsetupComplete }: Props) {
  const { val: p, set: setPanel } = panel;
  const [menuOpen, setMenuOpen] = useState(false);
  const settingsRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!menuOpen) return;
    const onPointerDown = (e: MouseEvent) => {
      if (
        settingsRef.current &&
        !settingsRef.current.contains(e.target as Node)
      ) {
        setMenuOpen(false);
      }
    };
    document.addEventListener("mousedown", onPointerDown);
    return () => document.removeEventListener("mousedown", onPointerDown);
  }, [menuOpen]);

  const load = useCallback(async () => {
    try {
      const s = await invoke<StatusResult>("prio_status", { repoPath });
      setPanel((prev) => ({ ...prev, status: s, lastResult: s.prio_result }));
    } catch (e) {
      setPanel((prev) => ({
        ...prev,
        lastResult: {
          status: "failure",
          message: String(e),
          logs: [],
        },
      }));
    }
  }, [repoPath, setPanel]);

  useEffect(() => {
    void load();
  }, [load]);

  const run = async (_command: string, fn: () => Promise<PrioResult>) => {
    setPanel((prev) => ({ ...prev, running: true }));
    try {
      const res = await fn();
      setPanel((prev) => ({ ...prev, lastResult: res }));
      await load();
      return res;
    } catch (e) {
      setPanel((prev) => ({
        ...prev,
        lastResult: {
          status: "failure",
          message: String(e),
          logs: [],
        },
      }));
    } finally {
      setPanel((prev) => ({ ...prev, running: false }));
    }
  };

  const applyBranch = (branch: string) =>
    run(`prio apply ${branch}`, () =>
      invoke("prio_apply", { repoPath, branches: [branch] })
    );

  const unapplyBranch = (branch: string) =>
    run(`prio unapply ${branch}`, () =>
      invoke("prio_unapply", { repoPath, branches: [branch] })
    );

  const push = () =>
    run(`prio push ${p.pushBranch}`, () =>
      invoke("prio_push", { repoPath, branch: p.pushBranch })
    );

  const pr = () =>
    run(`prio pr ${p.pushBranch}`, () =>
      invoke("prio_pr", { repoPath, branch: p.pushBranch })
    );

  const sync = () => run("prio sync", () => invoke("prio_sync", { repoPath }));

  const recover = () =>
    run("prio recover", () => invoke("prio_recover", { repoPath }));

  const stack = () =>
    run(`prio stack ${p.stackBranch} ${p.stackDeps}`, () =>
      invoke("prio_stack", {
        repoPath,
        branch: p.stackBranch,
        dependencies: p.stackDeps.split(/\s+/).filter(Boolean),
      })
    );

  const handleUnsetup = async () => {
    setMenuOpen(false);
    const confirmed = window.confirm(
      `Remove prio setup for this repository?\n\n${repoPath}\n\n` +
        "This archives .git/prio, renames the work branch, backs up the merge-conflicts clone, " +
        "and removes the repo from your prio configuration."
    );
    if (!confirmed) return;

    setPanel((prev) => ({ ...prev, running: true }));
    try {
      const res = await invoke<PrioResult>("prio_unsetup", { repoPath });
      setPanel((prev) => ({ ...prev, lastResult: res }));
      if (res.status !== "failure") {
        onUnsetupComplete();
      }
    } catch (e) {
      setPanel((prev) => ({
        ...prev,
        lastResult: {
          status: "failure",
          message: String(e),
          logs: [],
        },
      }));
    } finally {
      setPanel((prev) => ({ ...prev, running: false }));
    }
  };

  const onCommitDrop = (sha: string, destBranch: string) => {
    void run(`prio mv ${sha} ${destBranch}`, () =>
      invoke("prio_mv", {
        repoPath,
        commits: [sha],
        destination: destBranch,
        create: false,
      })
    );
  };

  const onBranchReorder = (fromIndex: number, toIndex: number) => {
    const applied = branches.filter((b) => b.applied);
    const lockedPrefix =
      p.status?.data.merge_conflict?.branches_merged.length ?? 0;
    if (fromIndex < lockedPrefix || toIndex < lockedPrefix) return;
    if (fromIndex === toIndex) return;
    const names = applied.map((b) => b.name);
    const [moved] = names.splice(fromIndex, 1);
    names.splice(toIndex, 0, moved);
    void run(`prio reorder ${names.join(" ")}`, () =>
      invoke("prio_reorder", { repoPath, branches: names })
    );
  };

  const branches: BranchInfo[] = p.status?.data.applied_branches ?? [];
  const unassigned: CommitInfo[] = p.status?.data.unassigned_commits ?? [];
  const appliedBranches = branches.filter((b) => b.applied);
  const unappliedBranches = branches.filter((b) => !b.applied);
  const lockedPrefix =
    p.status?.data.merge_conflict?.branches_merged.length ?? 0;

  const branchTitle = (b: BranchInfo) => {
    const pr = b.pr_number != null ? ` (PR #${b.pr_number})` : "";
    return `${b.name}${pr}`;
  };

  const statusNote = (b: BranchInfo) => {
    switch (b.apply_status) {
      case "merged":
        return "merged in prio-mc";
      case "conflict":
        return "merge conflict";
      case "pending":
        return "pending";
      default:
        return null;
    }
  };

  const conflict = p.status?.data.merge_conflict;

  return (
    <section className={styles.panel}>
      <div className={styles.headerRow}>
        <h2>Work area status for {repoPath}</h2>
        <div className={styles.settingsWrap} ref={settingsRef}>
          <button
            type="button"
            className={styles.gearBtn}
            aria-label="Repository settings"
            aria-expanded={menuOpen}
            aria-haspopup="menu"
            disabled={p.running}
            onClick={() => setMenuOpen((open) => !open)}
          >
            <span aria-hidden>⚙️</span>
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

      {conflict && (
        <div className={styles.conflictBanner} role="alert">
          <strong>⚠ Merge conflict in prio-mc</strong>
          {conflict.incoming_branch ? (
            <span>
              Merging <b>{conflict.incoming_branch}</b> into (
              {conflict.base_desc})
            </span>
          ) : (
            <span>A conflict is in progress in prio-mc.</span>
          )}
          <br />
          Resolve conflicts in: <b>{conflict.mc_path}</b>
          {conflict.merge_branch && (
            <>
              {" "}
              · branch <b>{conflict.merge_branch}</b>
            </>
          )}
          <code>git -C "{conflict.mc_path}" commit --no-edit</code>
          {conflict.incoming_branch && (
            <span className={styles.muted}>
              Or run <code>prio unapply {conflict.incoming_branch}</code> to
              discard and cancel.
            </span>
          )}
        </div>
      )}

      <div className={styles.columns}>
        {appliedBranches.map((b, index) => {
          const note = statusNote(b);
          return (
            <CommitList
              key={b.name}
              title={note ? `${branchTitle(b)} · ${note}` : branchTitle(b)}
              commits={b.commits ?? []}
              commitsDraggable={!p.running}
              onCommitDrop={(sha) => onCommitDrop(sha, b.name)}
              columnDraggable={!p.running && index > lockedPrefix}
              branchIndex={index}
              onBranchDrop={onBranchReorder}
              branchDropDisabled={p.running || index < lockedPrefix}
              headerExtra={
                <input
                  type="checkbox"
                  className={styles.applyCheck}
                  checked
                  disabled={p.running}
                  aria-label={`Unapply ${b.name}`}
                  onChange={() => void unapplyBranch(b.name)}
                />
              }
            />
          );
        })}
        {unappliedBranches.map((b) => (
          <CommitList
            key={b.name}
            title={branchTitle(b)}
            commits={b.commits ?? []}
            commitsDraggable={!p.running}
            onCommitDrop={(sha) => onCommitDrop(sha, b.name)}
            headerExtra={
              <input
                type="checkbox"
                className={styles.applyCheck}
                checked={false}
                disabled={p.running}
                aria-label={`Apply ${b.name}`}
                onChange={() => void applyBranch(b.name)}
              />
            }
          />
        ))}
        <CommitList
          title="Unassigned commits in work area"
          commits={unassigned}
          commitsDraggable={!p.running}
          onCommitDrop={(sha) => onCommitDrop(sha, ".")}
        />
      </div>

      <div className={styles.actions}>
        <input
          placeholder="branch to push/pr"
          value={p.pushBranch}
          onChange={(e) =>
            setPanel((prev) => ({ ...prev, pushBranch: e.target.value }))
          }
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
          placeholder="stacked branch"
          value={p.stackBranch}
          onChange={(e) =>
            setPanel((prev) => ({ ...prev, stackBranch: e.target.value }))
          }
        />
        <input
          placeholder="deps (space separated)"
          value={p.stackDeps}
          onChange={(e) =>
            setPanel((prev) => ({ ...prev, stackDeps: e.target.value }))
          }
        />
        <button type="button" onClick={() => void stack()} disabled={p.running}>
          Stack
        </button>
      </div>

      <div className={styles.actions}>
        <button type="button" onClick={() => void sync()} disabled={p.running}>
          Sync
        </button>
        <button
          type="button"
          onClick={() => void recover()}
          disabled={p.running}
        >
          Recover
        </button>
        <button type="button" onClick={() => void load()} disabled={p.running}>
          Refresh
        </button>
      </div>

      <LogViewer key={repoPath} result={p.lastResult} isRunning={p.running} />
    </section>
  );
}
