import styles from './CommandBadge.module.css'

interface Props {
  command: string
}

export function CommandBadge({ command }: Props) {
  const copy = () => {
    void navigator.clipboard.writeText(command)
  }

  return (
    <div className={styles.wrap}>
      <code className={styles.code}>{command}</code>
      <button type="button" className={styles.copy} onClick={copy} title="Copy CLI command">
        Copy
      </button>
    </div>
  )
}
