// The waiting sections that stack attached to the top of the composer.

import * as React from "react";
import { TaskStatusDot } from "@/components/atoms/TaskStatusDot";
import type { ActivitySubagent } from "@/domain/activity";
import { CloseIcon, FollowUpIcon } from "@/ui/icons";

export interface ComposerTrayProps {
  queued: string[];
  subagents: ActivitySubagent[];
  onRemoveQueued: (index: number) => void;
  onSelectSubagent: (nodeId: string) => void;
}

const QUEUED_PREVIEW = 3;
const SUBAGENT_PREVIEW = 3;

export function ComposerTray({ queued, subagents, onRemoveQueued, onSelectSubagent }: ComposerTrayProps) {
  const empty = queued.length === 0 && subagents.length === 0;
  return (
    <div className="composer-tray" data-empty={empty || undefined}>
      {queued.length > 0 && <QueuedSection queued={queued} onRemoveQueued={onRemoveQueued} />}
      {subagents.length > 0 && <SubagentsSection subagents={subagents} onSelectSubagent={onSelectSubagent} />}
    </div>
  );
}

/** A titled block in the tray; sections stack in the order they are given. */
export function ComposerTraySection({
  label,
  count,
  action,
  children,
}: {
  label: string;
  count?: number;
  action?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section className="composer-tray-section" aria-label={label}>
      <div className="composer-tray-header">
        <span className="composer-tray-label">{label}</span>
        {count !== undefined && <span className="composer-tray-count">{count}</span>}
        <span className="composer-tray-spacer" />
        {action}
      </div>
      {children}
    </section>
  );
}

function QueuedSection({ queued, onRemoveQueued }: Pick<ComposerTrayProps, "queued" | "onRemoveQueued">) {
  const [expanded, setExpanded] = React.useState(false);
  const collapsible = queued.length > QUEUED_PREVIEW;
  const shown = collapsible && !expanded ? queued.slice(0, QUEUED_PREVIEW) : queued;
  const action = collapsible ? (
    <button
      className="composer-tray-toggle"
      type="button"
      aria-expanded={expanded}
      onClick={() => setExpanded((value) => !value)}
    >
      {expanded ? "Show fewer" : `Show all ${queued.length}`}
    </button>
  ) : undefined;
  return (
    <ComposerTraySection label="Queued" count={queued.length} action={action}>
      <ul className="composer-tray-list">
        {shown.map((text, index) => (
          <li className="composer-tray-item" key={index}>
            <span className="composer-tray-item-icon" aria-hidden="true">
              <FollowUpIcon size={12} />
            </span>
            <span className="composer-tray-item-text" title={text}>
              {text}
            </span>
            <button
              className="icon-button composer-tray-remove"
              type="button"
              aria-label={`Remove queued message ${index + 1}`}
              title="Remove"
              onClick={() => onRemoveQueued(index)}
            >
              <CloseIcon size={13} />
            </button>
          </li>
        ))}
      </ul>
    </ComposerTraySection>
  );
}

function SubagentsSection({ subagents, onSelectSubagent }: Pick<ComposerTrayProps, "subagents" | "onSelectSubagent">) {
  const [expanded, setExpanded] = React.useState(false);
  const collapsible = subagents.length > SUBAGENT_PREVIEW;
  const shown = collapsible && !expanded ? subagents.slice(0, SUBAGENT_PREVIEW) : subagents;
  const action = collapsible ? (
    <button
      className="composer-tray-toggle"
      type="button"
      aria-expanded={expanded}
      onClick={() => setExpanded((value) => !value)}
    >
      {expanded ? "Show fewer" : `Show all ${subagents.length}`}
    </button>
  ) : undefined;
  return (
    <ComposerTraySection label="Subagents" count={subagents.length} action={action}>
      <ul className="composer-tray-list">
        {shown.map((subagent) => {
          const text = subagent.label ?? "Subagent";
          return (
            <li key={subagent.id}>
              <button
                className="composer-tray-item composer-tray-select"
                type="button"
                aria-label={`Show ${text} in the transcript`}
                title={`Show ${text} in the transcript`}
                onClick={() => onSelectSubagent(subagent.id)}
              >
                <span className="composer-tray-item-icon" aria-hidden="true">
                  <TaskStatusDot state="running" label="Running" decorative />
                </span>
                <span className="composer-tray-item-text" title={text}>
                  {text}
                </span>
              </button>
            </li>
          );
        })}
      </ul>
    </ComposerTraySection>
  );
}
