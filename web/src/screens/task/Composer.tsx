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
      return "Type your answer...";
    case "steer":
      return "Send a message...";
    case "steer-and-queue":
    case "queue":
      return "Add a follow-up...";
    case "resume":
      return routing.textRequired ? "What should happen next?" : "Send a message to continue...";
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

export interface ConversationComposerProps {
  routing: ConversationInputRouting;
  scope?: TaskScope;
  queued: string[];
  onSend: (request: ComposerRequest) => void;
  onRemoveQueued: (index: number) => void;
  thinkingToggle?: { active: boolean; onToggle: () => void };
}

const COMPOSER_MIN_HEIGHT = 44;
const COMPOSER_MAX_HEIGHT = 160;

export function ConversationComposer({ routing, scope, queued, onSend, onRemoveQueued, thinkingToggle }: ConversationComposerProps) {
  const [draft, setDraft] = React.useState("");
  const disabled = isSendDisabled(routing, draft);
  const inputRef = React.useRef<HTMLTextAreaElement>(null);
  const label = actionLabel(routing);

  const submit = (mode: ComposerSendMode) => {
    if (isSendDisabled(routing, draft)) return;
    const text = draft.trim();
    setDraft("");
    onSend({ mode, instruction: text === "" ? undefined : text });
  };

  const scopeLabel = scope ? (scope.write.length === 0 ? "Read only" : "Can edit") : undefined;
  const scopeHelp = scope
    ? scope.write.length === 0
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
                aria-label="Remove queued follow-up"
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
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
              event.preventDefault();
              submit("primary");
            }
          }}
          aria-label={placeholder(routing)}
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
              <span className="composer-scope" title={scopeHelp}>
                <span className="composer-scope-dot" aria-hidden="true">●</span>
                {scopeLabel}
              </span>
            )}
          </div>
          <div className="composer-footer-right">
            <span className="composer-hint" aria-hidden="true">⌘↵ to send</span>
            {routing.type === "steer-and-queue" && (
              <button
                className="composer-send-now"
                type="button"
                disabled={disabled}
                aria-label="Send now — interrupts the running worker"
                title="Send now — interrupts the running worker"
                onClick={() => submit("steer")}
              >
                Send now
              </button>
            )}
            <button className="composer-submit" type="submit" disabled={disabled} title={`${label} — ⌘/Ctrl+Enter`}>
              <SendIcon />
              {label}
            </button>
          </div>
        </div>
      </form>
      {routing.type === "steer-and-queue" && (
        <p className="composer-note">Send now interrupts the worker right away, instead of waiting its turn.</p>
      )}
      {routingsEqual(routing, { type: "resume", textRequired: false }) && (
        <p className="composer-note">Leave the message empty to continue the run.</p>
      )}
    </section>
  );
}
