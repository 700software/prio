import type { CommitInfo } from "../../types";
import styles from "./CommitList.module.css";

interface Props {
  title: string;
  commits: CommitInfo[];
  commitsDraggable?: boolean;
  onCommitDrop?: (sha: string) => void;
  headerExtra?: React.ReactNode;
  columnDraggable?: boolean;
  branchIndex?: number;
  onBranchDragStart?: (index: number) => void;
  onBranchDrop?: (fromIndex: number, toIndex: number) => void;
  branchDropDisabled?: boolean;
}

export function CommitList({
  title,
  commits,
  commitsDraggable,
  onCommitDrop,
  headerExtra,
  columnDraggable,
  branchIndex,
  onBranchDragStart,
  onBranchDrop,
  branchDropDisabled,
}: Props) {
  const droppable = !!onCommitDrop;
  const branchDroppable =
    columnDraggable && onBranchDrop != null && branchIndex != null;

  return (
    <div
      className={`${styles.column} ${droppable ? styles.droppable : ""}`}
      onDragOver={(e) => {
        if (droppable || branchDroppable) e.preventDefault();
      }}
      onDrop={(e) => {
        e.preventDefault();
        const sha = e.dataTransfer.getData("text/sha");
        if (sha && onCommitDrop) {
          onCommitDrop(sha);
          return;
        }
        if (branchDroppable && !branchDropDisabled) {
          const from = Number(e.dataTransfer.getData("text/branch-index"));
          if (!Number.isNaN(from)) onBranchDrop(from, branchIndex);
        }
      }}
    >
      <div
        className={`${styles.header} ${
          columnDraggable ? styles.draggableHeader : ""
        }`}
        draggable={columnDraggable}
        onDragStart={(e) => {
          if (branchIndex == null) return;
          e.dataTransfer.setData("text/branch-index", String(branchIndex));
          e.dataTransfer.effectAllowed = "move";
          onBranchDragStart?.(branchIndex);
        }}
        onDragOver={(e) =>
          branchDroppable && !branchDropDisabled && e.preventDefault()
        }
        onDrop={(e) => {
          if (!branchDroppable || branchDropDisabled) return;
          e.preventDefault();
          e.stopPropagation();
          const from = Number(e.dataTransfer.getData("text/branch-index"));
          if (!Number.isNaN(from)) onBranchDrop(from, branchIndex);
        }}
      >
        {headerExtra}
        <h3>{title}</h3>
      </div>
      <ul>
        {commits.length === 0 && <li className={styles.empty}>(none)</li>}
        {commits.map((c) => (
          <li
            key={c.sha}
            draggable={commitsDraggable && c.on_branch !== false}
            className={c.on_branch === false ? styles.pendingCommit : ""}
            onDragStart={(e) => {
              e.dataTransfer.setData("text/sha", c.sha);
              e.dataTransfer.effectAllowed = "move";
            }}
          >
            <code>{c.sha.slice(0, 7)}</code>
            <span>{c.message}</span>
            {c.on_branch === false && (
              <span
                className={styles.pendingBadge}
                title="Above baseline — not yet merged into branch"
              >
                pending
              </span>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}
