// The conversation composer: one input that routes to reply/steer/queue/resume.
// Ported from rust/crates/oga-ui/src/composer/mod.rs — keep behavior and copy identical.

import * as React from "react";
import type { TaskScope } from "@/bridge/types";
import { ChevronIcon, SendIcon } from "@/ui/icons";

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
}

const COMPOSER_MIN_HEIGHT = 44;
const COMPOSER_MAX_HEIGHT = 160;

export function ConversationComposer({ routing, scope, queued, onSend, onRemoveQueued, thinkingToggle }: ConversationComposerProps) {
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
          aria-describedby="composer-shortcut-hint"
        />
        <div className="composer-footer">
          <div className="composer-footer-left">
            {thinkingToggle && (
              <button
                className="composer-thinking-toggle"
                type="button"
                aria-pressed={thinkingToggle.active}
                onClick={thinkingToggle.onToggle}
              >
                {thinkingToggle.active ? "Hide thinking" : "Show thinking"}
              </button>
            )}
            {scopeLabel && (
              <span className={readOnlyScope ? "composer-scope composer-scope-read-only" : "composer-scope"} title={scopeHelp}>
                <span className="composer-scope-dot" aria-hidden="true">●</span>
                {scopeLabel}
              </span>
            )}
          </div>
          <div className="composer-footer-right">
            <span className="composer-hint" id="composer-shortcut-hint">
              {draft !== "" ? "Esc to clear · " : ""}{shortcut} to {label.toLowerCase()}
            </span>
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
            <button className="composer-submit" type="submit" disabled={disabled} title={`${label} — ${shortcut}`}>
              <SendIcon />
              {sending ? "Sending…" : label}
            </button>
          </div>
        </div>
      </form>
      {routingsEqual(routing, { type: "resume", textRequired: false }) && (
        <p className="composer-note">Leave the message empty to continue the run.</p>
      )}
    </section>
  );
}
