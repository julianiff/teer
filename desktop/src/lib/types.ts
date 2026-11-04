// The view model teer sends (`teer::session::view`), as JSON. Field names
// are Rust's; enums are their variant names.

export type Screen = "Queue" | "Unit" | "Flagged";
export type Level = "Same" | "Routine" | "Look" | "Attention";
export type Decision = "Pending" | "Accepted" | "Investigate";
export type Grouping = "File" | "Unit";
export type Prep = "NotRequested" | "Pending" | "Ready" | "Unavailable" | "Skipped";
export type Risk = "low" | "medium" | "high";
export type Marker = "added" | "changed" | "removed";
export type Side = "old" | "new";
export type DiveKind = "Recommend" | "Visualize" | "Behavior" | "Disagree" | "Tests";
export type RowKind = "Context" | "Removed" | "Added" | "Gap";
export type Class =
  | "Keyword"
  | "Function"
  | "Type"
  | "Variable"
  | "String"
  | "Comment"
  | "Constant";
export type TileId = "Advice" | "Ask" | "Tests" | "Behavior" | "Visualize" | "Outside";
export type Step =
  | "KeepReviewing"
  | "SendNotes"
  | "Yank"
  | "Commit"
  | "Done"
  | "RequestChanges"
  | "CommentOnly"
  | "Approve"
  | "ApproveWithComment";
export type DraftState = "Pending" | "Drafted" | "Template" | "Edited";

export interface View {
  screen: Screen;
  header: Header;
  bar: Bar;
  queue: Queue;
  preview: Card | null;
  unit: UnitScreen | null;
  flagged: FlaggedScreen | null;
  overlay: Overlay;
}

export interface Header {
  repo: string;
  subject: string;
  title: string | null;
  summary: string | null;
  no_agent: boolean;
  agent_note: string | null;
  preparing: { done: number; total: number } | null;
}

export interface Hint {
  keys: string;
  action: string;
  what: string;
}

export interface NoteEditor {
  id: string;
  name: string;
  text: string;
  max: number;
}

export interface Bar {
  pending: number;
  flagged: number;
  accepted: number;
  auto_accepted: number;
  finish_key: string;
  help_key: string;
  status: string;
  typed: string | null;
  follow: Hint[];
  command: string | null;
  commands: Hint[];
  visual: number | null;
  visual_actions: Hint[];
  note: NoteEditor | null;
  waiting: string | null;
}

export interface Queue {
  grouping: Grouping;
  boxes: LevelBox[];
  skipped: { path: string; why: string }[];
  auto_line: string | null;
  sel: number;
  show_auto: boolean;
}

export interface LevelBox {
  level: Level;
  title: string;
  files: FileGroup[];
}

export interface FileGroup {
  path: string;
  done: number;
  total: number;
  busy: boolean;
  flagged: boolean;
  rows: QueueRow[];
}

export interface QueueRow {
  pos: number;
  id: string;
  level: Level;
  icon: string;
  path: string;
  name: string;
  whole_file: boolean;
  depth: number;
  decision: Decision;
  state: string;
  note: string | null;
  disputed: boolean;
  noted: boolean;
  removed: boolean;
  preparing: boolean;
  selected: boolean;
  in_visual: boolean;
  changed: boolean;
}

export interface LineRef {
  side: Side;
  line: number;
}

export interface Statement {
  marker: Marker;
  symbol: string;
  text: string;
  risk: Risk;
  cites: string;
  evidence: LineRef[];
}

export interface Piece {
  text: string;
  class: Class | null;
}

export interface CodeRow {
  kind: RowKind;
  line: number | null;
  old: number | null;
  pieces: Piece[];
}

export interface Code {
  added: number;
  removed: number;
  rows: CodeRow[];
  notes: { after: number | null; statement: number }[];
  hunks: number[];
}

export interface Card {
  id: string;
  name: string;
  place: string;
  file: string;
  line: number;
  kind: string;
  status: string;
  level: Level;
  icon: string;
  level_title: string;
  decision: Decision;
  accepted_for_you: boolean;
  state: string;
  why: string[];
  reasons: string[];
  commits: { short: string; subject: string }[];
  agent_label: string | null;
  summary: string | null;
  nbc_reason: string | null;
  prep: Prep;
  prep_note: string | null;
  statements: Statement[];
  code: Code | null;
  note: { text: string; stale: boolean } | null;
  answers: { question: string; answer: string; shown: number }[];
  disputed: boolean;
}

export interface Cite {
  file: string;
  start: number;
  end: number;
}

export interface Shown {
  cite: Cite;
  why: string;
  code: string[];
  in_change: boolean;
  changed?: number[];
}

export type Outcome =
  | { kind: "value"; text: string }
  | { kind: "error"; text: string }
  | { kind: "missing" };

export interface ProbeCase {
  name: string;
  input: string;
  expect: "same" | "different";
  why: string;
  before: Outcome;
  after: Outcome;
  claim: number;
}

export type TestCheck =
  | { kind: "guarded" }
  | { kind: "unguarded" }
  | { kind: "fail_with_change"; detail: string }
  | { kind: "no_tests"; detail: string }
  | { kind: "not_run"; detail: string };

export interface Behavior {
  cases: ProbeCase[];
  command: string[];
  dir: string;
  claims?: string[];
  tests?: TestCheck;
  test_command?: string[];
}

export type Reach =
  | { how: "calls"; line: number }
  | { how: "through"; via: string; hops: number; line: number }
  | { how: "names" };

export interface Coverage {
  search?: { tests: { name: string; cite: Cite; reach: Reach }[]; total: number };
  judged: boolean;
  covered: { name: string; cite: Cite; checks: string }[];
  gaps: { case: string; danger: string; risk: Risk; evidence: Cite[] }[];
}

export interface DiveResult {
  text: string;
  sources: string[];
  visual?: unknown;
  behavior?: Behavior;
  coverage?: Coverage;
}

export interface SecondModel {
  label: string;
  head: string;
  verdicts: { agree: boolean; statement: string; why: string; cites: string }[];
  missed: { risk: Risk; statement: string; cites: string }[];
  failed: string | null;
}

export interface AnswerTile {
  kind: "Answer";
  question: DiveKind;
  label: string;
  result: DiveResult | null;
  svg: { svg: string; light: string; dark: string } | null;
  waiting: string | null;
  ask: Hint | null;
  none: string | null;
  second: SecondModel | null;
}

export type AskingState =
  | { kind: "Asking"; step: string | null; steps: number }
  | { kind: "Writing" }
  | { kind: "Answered" }
  | { kind: "Failed"; error: string }
  | { kind: "NotAnswered" };

export interface Exchange {
  index: number;
  question: string;
  state: AskingState;
  answer: string | null;
  related: {
    id: string;
    why: string;
    name: string;
    place: string;
    level: Level | null;
    link: number | null;
  }[];
  shown: { shown: Shown; link: number }[];
}

export interface AskTile {
  kind: "Ask";
  exchanges: Exchange[];
  draft: string;
  max: number;
  link: number | null;
  web_may_ask: boolean;
  label: string;
  tools: Hint[];
}

export type TileBody = AnswerTile | AskTile | { kind: "Outside"; shown: Shown[] };

export interface Tile {
  id: TileId;
  title: string;
  body: TileBody;
}

export interface UnitScreen {
  card: Card;
  second: SecondModel | null;
  tiles: Tile[];
  focus: TileId | null;
  zoom: boolean;
  typing: boolean;
  keys: { name: string; items: { keys: string; what: string; action: string; off: boolean }[] }[];
}

export interface FlaggedScreen {
  entries: Card[];
  sel: number;
}

export interface NoteItem {
  id: string;
  name: string;
  place: string;
  level: Level;
  decision: Decision;
  text: string;
  stale: boolean;
}

export interface JumpItem {
  id: string;
  name: string;
  place: string;
  level: Level;
  decision: Decision;
}

export interface Commit {
  sha: string;
  short: string;
  author: string;
  date: string;
  subject: string;
  body?: string;
  merge?: boolean;
}

export type Details =
  | { kind: "None" }
  | { kind: "Text"; text: string }
  | {
      kind: "Commit";
      files: [string, string][];
      branch: string | null;
      detached: boolean;
      others: number;
      message: string;
      draft: DraftState | null;
    }
  | {
      kind: "Post";
      head_check: string;
      needs_comment: boolean;
      body: string;
      inline: { path: string; line: number; old: boolean; body: string }[];
    };

export interface FinishView {
  reviewing: "Changes" | "Branch" | "Pr";
  subject: string;
  sel: number;
  steps: { step: Step; label: string; verb: string }[];
  not_shown: string | null;
  confirm: string | null;
  editing: string | null;
  details: Details;
  notes: NoteItem[];
}

export type JumpKind = "Find" | { Definition: { of: string } } | { Callers: { of: string } };

export type Overlay =
  | { kind: "None" }
  | ({ kind: "Help" } & { of: string; groups: { name: string; rows: { keys: string; what: string }[] }[] })
  | { kind: "ConfirmReset"; decisions: number }
  | ({ kind: "Note" } & NoteEditor)
  | { kind: "Notes"; sel: number; notes: NoteItem[] }
  | { kind: "ConfirmDeleteNote"; id: string; name: string }
  | { kind: "ConfirmWeb" }
  | {
      kind: "Commits";
      sel: number;
      unit: number | null;
      pr: { title: string; author: string; body?: string } | null;
      commits: Commit[];
      units: JumpItem[];
    }
  | ({ kind: "Finish" } & FinishView)
  | { kind: "Jump"; list: JumpKind; query: string; sel: number; items: JumpItem[] };

export type Click =
  | { kind: "Row"; pos: number }
  | { kind: "Open"; id: string }
  | { kind: "Tile"; id: TileId | null }
  | { kind: "Flagged"; i: number }
  | { kind: "Link"; i: number }
  | { kind: "Item"; i: number };

export interface Recent {
  path: string;
  name: string;
  opened: string;
}

export interface ListedPr {
  number: number;
  title: string;
  url: string;
  author: string;
  draft: boolean;
  review_decision?: string;
  updated_at?: string;
  head: string;
}

export interface Listing {
  repo: { remote: string; host: string; owner: string; repo: string };
  needs_review: ListedPr[];
  assigned: ListedPr[];
  mine: ListedPr[];
  truncated: boolean;
}

export interface Loaded {
  opening: string | null;
  view: View | null;
}
