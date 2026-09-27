// The conversation composer routes one input to reply, steer, queue, or resume.

import * as React from "react";
import type { Task, TaskScope } from "@/bridge/types";
import { TaskStatusDot } from "@/components/atoms/TaskStatusDot";
import { ChevronIcon, ReturnIcon } from "@/ui/icons";
import { taskStatusLabel } from "./format";

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
  task: Task;
  focusRequest?: { taskId: string; nonce: number };
  onFocusRequestConsumed: (nonce: number) => void;
}

const COMPOSER_MIN_HEIGHT = 44;
const COMPOSER_MAX_HEIGHT = 160;

export function ConversationComposer({
  routing,
  scope,
  queued,
  onSend,
  onRemoveQueued,
  task,
  focusRequest,
  onFocusRequestConsumed,
}: ConversationComposerProps) {
  const [draft, setDraft] = React.useState("");
  const [sending, setSending] = React.useState(false);
  const disabled = isSendDisabled(routing, draft) || sending;
  const inputRef = React.useRef<HTMLTextAreaElement>(null);
  const label = actionLabel(routing);
  const shortcut = submitShortcut();

  React.useEffect(() => {
    if (!focusRequest || focusRequest.taskId !== task.id) return;
    const input = inputRef.current;
    if (!input) return;
    input.focus();
    if (document.activeElement === input) onFocusRequestConsumed(focusRequest.nonce);
  }, [focusRequest, onFocusRequestConsumed, task.id]);

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
          <TaskStatusDot state={task.state} label={taskStatusLabel(task)} />
        </div>
      </div>
      <p className="composer-note">
        {routingsEqual(routing, { type: "resume", textRequired: false }) && "Leave the message empty to continue the run."}
      </p>
    </section>
  );
}
