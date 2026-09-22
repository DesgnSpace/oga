// The conversation composer: one input that routes to reply/steer/queue/resume.
// Ported from rust/crates/oga-ui/src/composer/mod.rs — keep behavior and copy identical.

import * as React from "react";
import { createPortal } from "react-dom";
import type { Task, TaskEventView, TaskScope } from "@/bridge/types";
import { CheckIcon, ChevronIcon, PlusIcon, ReturnIcon } from "@/ui/icons";
import { effortDisplay } from "./format";
import { TaskMetadata } from "./TaskMetadata";

const MENU_MARGIN = 8;

export type ComposerSendMode = "primary" | "steer";

export type ConversationInputRouting =
  | { type: "reply"; question: string }
  | { type: "steer" }
  | { type: "steer-and-queue" }
  | { type: "queue" }
  | { type: "resume"; textRequired: boolean }
  | { type: "none" };

export function routingForState(
  state: string,
  steerable: boolean,
  question: string | undefined,
): ConversationInputRouting {
  switch (state) {
    case "needs_input":
      return { type: "reply", question: question ?? "" };
    case "running":
      return steerable ? { type: "steer-and-queue" } : { type: "queue" };
    case "queued":
    case "answered":
      return { type: "queue" };
    case "preparing_checkout":
    case "removing_checkout":
      return { type: "none" };
    case "pending":
    case "failed":
    case "cancelled":
    case "blocked":
      return { type: "resume", textRequired: false };
    case "completed":
      return { type: "resume", textRequired: true };
    default:
      return { type: "none" };
  }
}

function placeholder(routing: ConversationInputRouting): string {
  switch (routing.type) {
    case "reply":
      return "Type your answer…";
    case "steer":
      return "Send a message…";
    case "steer-and-queue":
    case "queue":
      return "Add a follow-up…";
    case "resume":
      return routing.textRequired ? "What should happen next?" : "Send a message to continue…";
    case "none":
      return "";
  }
}

function actionLabel(routing: ConversationInputRouting): string {
  switch (routing.type) {
    case "reply":
      return "Reply";
    case "steer":
      return "Send";
    case "steer-and-queue":
    case "queue":
      return "Queue";
    case "resume":
      return routing.textRequired ? "Send" : "Continue";
    case "none":
      return "Send";
  }
}

function routingsEqual(a: ConversationInputRouting, b: ConversationInputRouting): boolean {
  if (a.type !== b.type) return false;
  if (a.type === "resume" && b.type === "resume") return a.textRequired === b.textRequired;
  return true;
}

function submitShortcut(): string {
  const platform = typeof navigator === "undefined" ? "" : `${navigator.platform} ${navigator.userAgent}`;
  return /Mac/.test(platform) ? "⌘↵" : "Ctrl+↵";
}

export function isSendDisabled(routing: ConversationInputRouting, draft: string): boolean {
  const empty = draft.trim() === "";
  switch (routing.type) {
    case "steer":
    case "steer-and-queue":
    case "queue":
    case "reply":
      return empty;
    case "resume":
      return routing.textRequired && empty;
    case "none":
      return true;
  }
}

export function isResume(routing: ConversationInputRouting): boolean {
  return routing.type === "resume";
}

export interface ComposerRequest {
  mode: ComposerSendMode;
  instruction: string | undefined;
}

/** Return false to keep the draft text (the send failed); anything else clears it. */
export type ComposerSend = (request: ComposerRequest) => Promise<boolean | void> | boolean | void;

export interface ConversationComposerProps {
  routing: ConversationInputRouting;
  scope?: TaskScope;
  queued: string[];
  onSend: ComposerSend;
  onRemoveQueued: (index: number) => void;
  thinkingToggle?: { active: boolean; onToggle: () => void };
  task: Task;
  events: TaskEventView[];
  contextWindow: number | undefined;
}

const COMPOSER_MIN_HEIGHT = 44;
const COMPOSER_MAX_HEIGHT = 160;

export function ConversationComposer({
  routing,
  scope,
  queued,
  onSend,
  onRemoveQueued,
  thinkingToggle,
  task,
  events,
  contextWindow,
}: ConversationComposerProps) {
  const [menuOpen, setMenuOpen] = React.useState(false);
  const menuTriggerRef = React.useRef<HTMLButtonElement>(null);
  const [draft, setDraft] = React.useState("");
  const [sending, setSending] = React.useState(false);
  const disabled = isSendDisabled(routing, draft) || sending;
  const inputRef = React.useRef<HTMLTextAreaElement>(null);
  const label = actionLabel(routing);
  const shortcut = submitShortcut();

  const submit = async (mode: ComposerSendMode) => {
    if (isSendDisabled(routing, draft) || sending) return;
    const text = draft.trim();
    setSending(true);
    try {
      const cleared = await onSend({ mode, instruction: text === "" ? undefined : text });
      if (cleared !== false) setDraft("");
    } finally {
      setSending(false);
    }
  };

  const readOnlyScope = scope ? scope.write.length === 0 : false;
  const scopeLabel = scope ? (readOnlyScope ? "Read only" : "Can edit") : undefined;
  const scopeHelp = scope
    ? readOnlyScope
      ? "This run can read files but not change them."
      : "This run can change files."
    : undefined;
  const effort = effortDisplay(task);
  const running = task.state === "running";
  const branch = task.worktree?.branch ?? task.branch;

  // Resetting height first makes scrollHeight reflect only the content, so it shrinks back too.
  React.useLayoutEffect(() => {
    const input = inputRef.current;
    if (!input) return;
    input.style.height = "0px";
    const next = Math.min(input.scrollHeight, COMPOSER_MAX_HEIGHT);
    input.style.height = `${Math.max(next, COMPOSER_MIN_HEIGHT)}px`;
    input.style.overflowY = input.scrollHeight > COMPOSER_MAX_HEIGHT ? "auto" : "hidden";
  }, [draft]);

  return (
    <section className="conversation-composer" aria-label="Task conversation">
      {queued.length > 0 && (
        <div className="queued-follow-ups" aria-label="Queued follow-ups">
          {queued.map((text, index) => (
            <div className="queued-follow-up" key={index}>
              <span className="queued-follow-up-marker" aria-hidden="true">
                <ChevronIcon size={11} />
              </span>
              <span className="queued-follow-up-text">{text}</span>
              <button
                className="text-button queued-follow-up-remove"
                type="button"
                aria-label={`Remove follow-up ${index + 1}`}
                onClick={() => onRemoveQueued(index)}
              >
                Remove
              </button>
            </div>
          ))}
        </div>
      )}
      <form
        className="composer-card"
        onSubmit={(event) => {
          event.preventDefault();
          submit("primary");
        }}
      >
        <textarea
          ref={inputRef}
          className="composer-input"
          rows={1}
          placeholder={placeholder(routing)}
          value={draft}
          readOnly={sending}
          aria-busy={sending}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
              event.preventDefault();
              void submit("primary");
              return;
            }
            if (event.key === "Escape" && draft !== "") {
              event.preventDefault();
              setDraft("");
            }
          }}
          aria-label={placeholder(routing)}
        />
        <button
          className={`composer-send${draft.trim() !== "" ? " composer-send-active" : ""}`}
          type="submit"
          disabled={disabled}
          aria-label={sending ? "Sending…" : `${label} — ${shortcut}`}
          title={sending ? "Sending…" : `${label} — ${shortcut}`}
        >
          <ReturnIcon size={15} />
        </button>
      </form>
      <div className="composer-controls">
        <div className="composer-controls-left">
          <div className="composer-menu-anchor">
            <button
              ref={menuTriggerRef}
              className="icon-button composer-menu-trigger"
              type="button"
              aria-label="More options"
              title="More options"
              aria-haspopup="menu"
              aria-expanded={menuOpen}
              onClick={() => setMenuOpen((value) => !value)}
            >
              <PlusIcon size={14} />
            </button>
            {menuOpen && (
              <ComposerMenu
                triggerRef={menuTriggerRef}
                thinkingToggle={thinkingToggle}
                task={task}
                events={events}
                contextWindow={contextWindow}
                onClose={() => setMenuOpen(false)}
              />
            )}
          </div>
          {scopeLabel && (
            <span className="composer-scope-picker" title={scopeHelp}>
              {scopeLabel}
            </span>
          )}
          {branch && (
            <span className="composer-branch" title={branch}>
              {branch}
            </span>
          )}
        </div>
        <div className="composer-controls-right">
          {routing.type === "steer-and-queue" && (
            <button
              className="composer-send-now"
              type="button"
              disabled={disabled}
              aria-label="Send now — interrupts the running worker"
              title="Send now — interrupts the running worker"
              onClick={() => void submit("steer")}
            >
              Send now
            </button>
          )}
          <span className="composer-run-facts">
            <span className="composer-run-model" title={task.model}>
              {task.model}
            </span>
            {effort && (
              <span className="composer-run-effort" title={effort.title}>
                {effort.label.charAt(0).toUpperCase() + effort.label.slice(1)}
              </span>
            )}
          </span>
          {running && <span className="composer-run-spinner" role="img" aria-label="Running" title="Running" />}
        </div>
      </div>
      {routingsEqual(routing, { type: "resume", textRequired: false }) && (
        <p className="composer-note">Leave the message empty to continue the run.</p>
      )}
    </section>
  );
}

interface ComposerMenuProps {
  triggerRef: React.RefObject<HTMLButtonElement | null>;
  thinkingToggle?: { active: boolean; onToggle: () => void };
  task: Task;
  events: TaskEventView[];
  contextWindow: number | undefined;
  onClose: () => void;
}

interface MenuPlacement {
  bottom: number;
  left: number;
  maxHeight: number;
}

const MENU_WIDTH = 240;

/** Anchors the panel's bottom edge just above the trigger, using a fixed panel width. */
function computePlacement(trigger: HTMLElement): MenuPlacement {
  const rect = trigger.getBoundingClientRect();
  const viewportWidth = window.innerWidth;
  const viewportHeight = window.innerHeight;
  return {
    bottom: viewportHeight - rect.top + MENU_MARGIN,
    left: Math.max(MENU_MARGIN, Math.min(rect.left, viewportWidth - MENU_WIDTH - MENU_MARGIN)),
    maxHeight: Math.max(0, rect.top - MENU_MARGIN * 2),
  };
}

/**
 * The composer's own popover: opens above the `+` trigger, with a "Show
 * thinking" toggle and a "Details" row that swaps in the same task metadata
 * list shown elsewhere. Portals to the body because the composer's own
 * scroll container clips anything positioned outside its box.
 */
function ComposerMenu({ triggerRef, thinkingToggle, task, events, contextWindow, onClose }: ComposerMenuProps) {
  const [showDetails, setShowDetails] = React.useState(false);
  const menuRef = React.useRef<HTMLDivElement>(null);
  const [placement, setPlacement] = React.useState<MenuPlacement>(() =>
    triggerRef.current ? computePlacement(triggerRef.current) : { bottom: MENU_MARGIN, left: MENU_MARGIN, maxHeight: 300 },
  );
  const onCloseRef = React.useRef(onClose);
  onCloseRef.current = onClose;

  React.useEffect(() => {
    const update = () => {
      const trigger = triggerRef.current;
      if (!trigger) return;
      setPlacement(computePlacement(trigger));
    };
    window.addEventListener("resize", update);
    document.addEventListener("scroll", update, true);
    return () => {
      window.removeEventListener("resize", update);
      document.removeEventListener("scroll", update, true);
    };
  }, [triggerRef]);

  React.useEffect(() => {
    const firstItem = menuRef.current?.querySelector<HTMLButtonElement>("button:not(:disabled)");
    firstItem?.focus();

    const closeOnPointer = (event: PointerEvent) => {
      // SAFETY: pointer events always target a Node in the DOM tree.
      const target = event.target as Node | null;
      if (target && (menuRef.current?.contains(target) || triggerRef.current?.contains(target))) return;
      onCloseRef.current();
    };
    document.addEventListener("pointerdown", closeOnPointer);
    return () => document.removeEventListener("pointerdown", closeOnPointer);
  }, [triggerRef, showDetails]);

  const handleKeyDown = (event: React.KeyboardEvent) => {
    if (event.key === "Escape") {
      event.preventDefault();
      if (showDetails) {
        setShowDetails(false);
        return;
      }
      onClose();
      triggerRef.current?.focus();
      return;
    }
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const items = Array.from(menuRef.current?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? []);
      const currentIndex = items.findIndex((item) => item === document.activeElement);
      const delta = event.key === "ArrowDown" ? 1 : -1;
      items[(currentIndex + delta + items.length) % items.length]?.focus();
    }
  };

  if (globalThis.document === undefined) return null;

  return createPortal(
    <div
      ref={menuRef}
      className="menu-panel composer-menu"
      role="menu"
      style={{ bottom: placement.bottom, left: placement.left, maxHeight: placement.maxHeight }}
      onKeyDown={handleKeyDown}
    >
      {showDetails ? (
        <>
          <button type="button" className="menu-item composer-menu-back" onClick={() => setShowDetails(false)}>
            <ChevronIcon size={12} className="composer-menu-back-icon" />
            <span className="menu-item-label">Details</span>
          </button>
          <div className="composer-menu-details">
            <TaskMetadata task={task} events={events} contextWindow={contextWindow} />
          </div>
        </>
      ) : (
        <>
          {thinkingToggle && (
            <button
              type="button"
              role="menuitemcheckbox"
              aria-checked={thinkingToggle.active}
              className="menu-item"
              onClick={() => {
                thinkingToggle.onToggle();
                onClose();
              }}
            >
              <span className="menu-item-icon" aria-hidden="true">
                {thinkingToggle.active && <CheckIcon size={12} />}
              </span>
              <span className="menu-item-label">Show thinking</span>
            </button>
          )}
          <button type="button" role="menuitem" className="menu-item" onClick={() => setShowDetails(true)}>
            <span className="menu-item-icon" aria-hidden="true" />
            <span className="menu-item-label">Details</span>
            <ChevronIcon size={12} className="menu-item-chevron" />
          </button>
        </>
      )}
    </div>,
    document.body,
  );
}
