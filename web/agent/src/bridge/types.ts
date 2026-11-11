// The agent as the Spooky server describes it (spooky_core::Agent, serialized by serde).
// Everything in it that came from a model, a tool or a file is untrusted text: draw it,
// never act on it.

export type Role = "User" | "Assistant" | "Thinking" | "Tool" | "Notice";
export type ToolState = "Running" | "Done" | "Failed";
export type DialogKind = "Confirm" | "Select" | "Input";

export interface Tool {
  name: string;
  detail: string;
  state: ToolState;
  output: string;
  /** What an edit changed: lines of "<sign><number>|<text>", sign a space, - or +. */
  diff?: string;
}

export interface Message {
  id: number;
  role: Role;
  text: string;
  tool?: Tool;
}

export interface Dialog {
  id: string;
  kind: DialogKind;
  title: string;
  message: string;
  options: string[];
}

export interface ModelRole {
  role: string;
  model: string;
  thinking: string | null;
}

export interface UsageWindow {
  name: string;
  account: string;
  left: number;
  resets: string;
}

export interface UsageProvider {
  name: string;
  notes: string[];
  windows: UsageWindow[];
}

export interface Agent {
  backend: string;
  busy: boolean;
  ended: string | null;
  dropped: number;
  messages: Message[];
  dialogs: Dialog[];
  model: string | null;
  thinking: string | null;
  models: string[];
  levels: string[];
  context: number | null;
  roles: ModelRole[];
  usage: UsageProvider[] | null;
}

/** What the desktop app pushes: the agent pane on show, and the host it runs on. */
/** A file the next message carries. */
export interface Attachment {
  named: string;
  name: string;
  /** The small picture as a data: URI, when the file is one. */
  preview: string | null;
}

export interface View {
  pane: number | null;
  agent: Agent | null;
  attachments: Attachment[];
  platform: string;
}

export type Answer = { Confirm: boolean } | { Value: string } | "Cancel";
