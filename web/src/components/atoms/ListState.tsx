import type { ReactNode } from "react";

type EmptyStateProps = {
  title: string;
  hint?: string;
  action?: ReactNode;
  className?: string;
};

export function EmptyState({ title, hint, action, className }: EmptyStateProps) {
  return (
    <div className={`list-state list-state-empty${className ? ` ${className}` : ""}`}>
      <strong>{title}</strong>
      {hint ? <p>{hint}</p> : null}
      {action ? <div className="list-state-action">{action}</div> : null}
    </div>
  );
}

export function LoadingState({ label, className }: { label: string; className?: string }) {
  return (
    <div
      className={`list-state list-state-loading${className ? ` ${className}` : ""}`}
      role="status"
      aria-busy="true"
    >
      <span className="list-state-spinner" aria-hidden="true" />
      <span>{label}</span>
    </div>
  );
}
