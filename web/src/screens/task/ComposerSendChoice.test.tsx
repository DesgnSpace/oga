import { afterEach, describe, expect, it } from "bun:test";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { setTransport } from "@/bridge/transport";
import type { Task } from "@/bridge/types";
import { TaskControls } from "./Actions";
import { ConversationComposer, type ComposerRequest } from "./Composer";

const task = {
  id: "task-1",
  state: "running",
  prompt: "Tidy the parser",
  createdAt: "2026-09-26T10:00:00Z",
  updatedAt: "2026-09-26T10:00:00Z",
} as Task;

function composer(onSend: (request: ComposerRequest) => void) {
  return render(
    <ConversationComposer
      routing={{ type: "steer-and-queue" }}
      queued={[]}
      subagents={[]}
      onSend={(request) => {
        onSend(request);
        return true;
      }}
      onRemoveQueued={() => {}}
      task={task}
      onFocusRequestConsumed={() => {}}
    />,
  );
}

function draft(value: string) {
  const input = screen.getByPlaceholderText(/Send a message|Add a follow-up/) as HTMLTextAreaElement;
  fireEvent.change(input, { target: { value } });
  return input;
}

function sendButton(container: HTMLElement) {
  const button = container.querySelector("button.composer-send");
  if (!button) throw new Error("no send button");
  return button;
}

function choice(name: "Send now" | "Queue") {
  const group = screen.getByRole("group", { name: "Choose when your message goes" });
  return within(group).getByRole("button", { name: new RegExp(`^${name}`) });
}

describe("the composer send choice while a run is active", () => {
  afterEach(cleanup);

  it("sends now by default, so a first message interrupts the run instead of queueing behind it", async () => {
    const modes: string[] = [];
    const { container } = composer((request) => {
      modes.push(request.mode);
    });
    draft("hold on");
    fireEvent.click(sendButton(container));

    await waitFor(() => expect(modes).toEqual(["steer"]));
  });

  it("remembers the queue choice for the next message in the same view", async () => {
    const modes: string[] = [];
    const { container } = composer((request) => {
      modes.push(request.mode);
    });
    fireEvent.click(choice("Queue"));
    draft("first");
    fireEvent.click(sendButton(container));
    await waitFor(() => expect(modes).toEqual(["primary"]));

    draft("second");
    fireEvent.click(sendButton(container));
    await waitFor(() => expect(modes).toEqual(["primary", "primary"]));
  });

  it("sends the choice on the submit shortcut and the other action on the alternate one", async () => {
    const modes: string[] = [];
    composer((request) => {
      modes.push(request.mode);
    });
    const input = draft("hold on");
    fireEvent.keyDown(input, { key: "Enter", altKey: true });
    await waitFor(() => expect(modes).toEqual(["primary"]));

    draft("wait");
    fireEvent.keyDown(screen.getByPlaceholderText(/Send a message|Add a follow-up/), {
      key: "Enter",
      metaKey: true,
    });
    // The one-shot alternate send leaves the remembered choice alone.
    await waitFor(() => expect(modes).toEqual(["primary", "steer"]));
  });

  it("offers no choice when no run is active", () => {
    render(
      <ConversationComposer
        routing={{ type: "queue" }}
        queued={[]}
        subagents={[]}
        onSend={() => true}
        onRemoveQueued={() => {}}
        task={{ ...task, state: "queued" }}
        onFocusRequestConsumed={() => {}}
      />,
    );
    expect(screen.queryByRole("group", { name: "Choose when your message goes" })).toBeNull();
  });
});

function serveSteer(queued: boolean | undefined) {
  setTransport({
    invoke: async (command: string, args?: Record<string, unknown>) => {
      if (command !== "broker_call") throw { message: `no handler for ${command}` };
      const { call } = args?.call as { call: string };
      if (call === "steerTask") return { id: "task-1", state: "running", queued } as never;
      throw { message: `no handler for ${call}` };
    },
    listen: () => {},
  });
}

function controls() {
  return render(
    <TaskControls
      task={task}
      events={[]}
      subagents={[]}
      onChanged={() => {}}
      onFocusRequestConsumed={() => {}}
    />,
  );
}

describe("a send-now the run could not take", () => {
  afterEach(cleanup);

  it("says the message was queued instead, so the outcome matches what was pressed", async () => {
    serveSteer(true);
    const { container } = controls();
    // A running task offers both actions; Send now is the default.
    const input = screen.getByPlaceholderText(/Send a message|Add a follow-up/);
    fireEvent.change(input, { target: { value: "hold on" } });
    fireEvent.click(sendButton(container));

    await screen.findByText("Couldn't reach the run — queued until it finishes.");
  });

  it("stays quiet when the run took the message", async () => {
    serveSteer(undefined);
    const { container } = controls();
    const input = screen.getByPlaceholderText(/Send a message|Add a follow-up/);
    fireEvent.change(input, { target: { value: "hold on" } });
    fireEvent.click(sendButton(container));

    await waitFor(() => expect(screen.queryByText(/queued until this run finishes/)).toBeNull());
  });
});
