// The waiting sections that stack attached to the top of the composer.

import * as React from "react";
import { CloseIcon, FollowUpIcon } from "@/ui/icons";

export interface ComposerTrayProps {
  queued: string[];
  onRemoveQueued: (index: number) => void;
}

const QUEUED_PREVIEW = 3;

export function ComposerTray({ queued, onRemoveQueued }: ComposerTrayProps) {
  const empty = queued.length === 0;
  return (
    <div className="composer-tray" data-empty={empty || undefined}>
      {!empty && <QueuedSection queued={queued} onRemoveQueued={onRemoveQueued} />}
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

function QueuedSection({ queued, onRemoveQueued }: ComposerTrayProps) {
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
