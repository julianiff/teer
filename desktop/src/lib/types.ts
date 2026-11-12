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
export type DiveKind = "Recommend" | "Visualize" | "Behavior" | "Disagree" | "Tests" | "Security";
export type RowKind = "Context" | "Removed" | "Added" | "Gap";
export type Class =
  | "Keyword"
  | "Function"
  | "Type"
  | "Variable"
  | "String"
  | "Comment"
  | "Constant";
export type TileId = "Advice" | "Ask" | "Tests" | "Behavior" | "Visualize" | "Security" | "Outside";
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
  /** What buttons press, by action id: each one's key, what it does,
   *  whether it can. The window puts them where they act. */
  actions: Record<string, ActionView>;
  /** What the open panel's keys do, for buttons on it. */
  panel: Hint[];
}

/** An action a button presses: its key (empty when unbound) and what it
 *  does, in the screen's words. */
export interface ActionView {
  keys: string;
  what: string;
  /** Needs an agent, and there is none. */
  off: boolean;
  /** A toggle's state (listed by file, zoomed); null for the rest. */
  on: boolean | null;
}

export interface Header {
  repo: string;
  home: string;
  pr: number | null;
  subject: string;
  title: string | null;
  summary: string | null;
  no_agent: boolean;
  agent_note: string | null;
  preparing: { done: number; total: number } | null;
  /** The commits reviewed. */
  commits: number;
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
  /** Units with a note. */
  notes: number;
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
  /** The attack-surface card; absent while the security lens is off. */
  surface?: Surface;
}

export interface LevelBox {
  level: Level;
  title: string;
  files: FileGroup[];
  /** The security box (`zs`, by exposure): its units have security facts
   *  or findings; `level` is the highest of theirs. */
  security?: boolean;
}

/** The attack-surface card: a row per class the change touches. No rows:
 *  nothing found, which never means safe. */
export interface Surface {
  rows?: SurfaceRow[];
  /** What teer checked: `auth · entry points · …`. */
  checked: string;
  /** What nobody checked. */
  not: string;
}

export interface SurfaceRow {
  /** The security class, as `teer::security::Class` names it. */
  class:
    | "authorization"
    | "entry_point"
    | "injection"
    | "secret"
    | "exposure"
    | "supply_chain"
    | "ci"
    | "config"
    | "crypto"
    | "llm";
  /** `entry points`, `sinks`, `checks`. */
  label: string;
  /** Most severe first. */
  items: SurfaceItem[];
}

export interface SurfaceItem {
  /** The unit it is on, by id. */
  unit: string;
  text: string;
  weight: "note" | "look" | "attention";
  /** The agent's finding, standing; else a fact teer found. */
  finding: boolean;
  /** A check that is gone. */
  gone: boolean;
  /** How many units it is on. */
  units: number;
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
  /** Hunks of a change the agent proposed that aren't put in yet. */
  proposed?: number;
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

/** What a security fact or finding is about (`security::Class`). */
export type SecurityClass =
  | "authorization"
  | "entry_point"
  | "injection"
  | "secret"
  | "exposure"
  | "supply_chain"
  | "ci"
  | "config"
  | "crypto"
  | "llm";

/** A security fact teer found in the change without an agent. */
export interface Fact {
  class: SecurityClass;
  weight: "note" | "look" | "attention";
  text: string;
  side: Side;
  lines?: { start: number; end: number };
  subject?: string;
  /** Absent: teer's own; else another tool's finding. */
  origin?: { from: "import"; tool: string; rule: string };
}

/** A call on a finding's path. */
export interface Hop {
  symbol: string;
  cite: Cite;
}

/** The second model's word on a finding. */
export interface SecondCheck {
  agree: boolean;
  why: string;
  evidence?: Cite;
}

/** What running a finding's attacker case old against new showed. */
export type Proof =
  | { state: "proven"; case: string; before: string; after: string }
  | { state: "not_shown"; case: string; before: string; after: string }
  | { state: "unproven"; why: string };

/** One way the change can be abused, with the path that shows it. */
export interface Finding {
  class: SecurityClass;
  severity: Risk;
  confidence: Risk;
  title: string;
  source: Cite;
  path: Hop[];
  sink: Cite;
  missing: string;
  exploit: string;
  /** Every call on the path is one teer's graph knows. */
  path_verified: boolean;
  second?: SecondCheck;
  proof?: Proof;
}

/** The Security tile's answer. "No finding" never means safe. */
export interface Audit {
  findings: Finding[];
  holds: { what: string; cite: Cite }[];
  checked: SecurityClass[];
  not_checked: string[];
  dropped: number;
}

export interface DiveResult {
  text: string;
  sources: string[];
  visual?: unknown;
  behavior?: Behavior;
  coverage?: Coverage;
  audit?: Audit;
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
  /** The Security tile: the facts teer found in the change itself. */
  facts?: Fact[];
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
  /** The change the agent proposes, or that one is on its way. */
  proposal?: ProposalView;
}

export type HunkState = "proposed" | "applied" | "suggested";

/** One proposed hunk, in a box like the code change's. */
export interface HunkView {
  /** `src/x.php:3-5`. */
  place: string;
  why: string;
  /** The file still parses with it. */
  parses: boolean;
  state: HunkState;
  code: Code;
}

/** A change the agent proposes for the unit. */
export interface ProposalView {
  /** The finding's title, `your note`, `the advice`. */
  answers: string | null;
  severity: Risk | null;
  /** Who wrote it: `agent (opus)`. */
  label: string;
  summary: string;
  second: SecondCheck | null;
  dropped: number;
  hunks: HunkView[];
  /** The hunk `fix.apply` puts in. */
  current: number | null;
  /** A change is on its way, and what it is doing. */
  waiting: string | null;
  /** On a pull request the hunks become suggestions. */
  suggest: boolean;
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

/** An agent session running in the repository, to send a tile's result
 *  to (`s`). */
export interface SendTarget {
  agent: "ClaudeCode" | "Codex" | "Pi";
  id: string;
  /** Its name as the agent shows it, else its id cut short. */
  name: string;
  cwd: string;
  /** `idle`, `busy`, or empty when the agent doesn't say. */
  status: string;
  /** Last active, in ms since the epoch; 0 when unknown. */
  active_at: number;
  socket?: string;
}

/** The sessions to send a tile's result to: `what` it is, the message,
 *  the one selected. `inside` is where each runs relative to the
 *  repository ("" at its root), `wrote` the one that wrote the change;
 *  with `pi_install` a last row (index `targets.length`) installs teer's
 *  pi extension, after the question `confirm`. */
export interface SendToView {
  what: string;
  targets: SendTarget[];
  sel: number;
  text: string;
  inside: string[];
  wrote: number | null;
  pi_install: boolean;
  confirm: string | null;
}

export type Overlay =
  | { kind: "None" }
  | ({ kind: "Help" } & { of: string; groups: { name: string; rows: { keys: string; what: string; each: Hint[] }[] }[] })
  | { kind: "ConfirmReset"; decisions: number }
  | ({ kind: "Note" } & NoteEditor)
  | { kind: "Notes"; sel: number; notes: NoteItem[] }
  | { kind: "ConfirmDeleteNote"; id: string; name: string }
  | { kind: "ConfirmWeb" }
  | { kind: "ConfirmApply"; question: string }
  | {
      kind: "Commits";
      sel: number;
      unit: number | null;
      pr: { title: string; author: string; body?: string } | null;
      commits: Commit[];
      units: JumpItem[];
    }
  | ({ kind: "Finish" } & FinishView)
  | { kind: "Jump"; list: JumpKind; query: string; sel: number; items: JumpItem[] }
  | ({ kind: "SendTo" } & SendToView);

export type Click =
  | { kind: "Row"; pos: number }
  | { kind: "Open"; id: string }
  | { kind: "Tile"; id: TileId | null }
  | { kind: "CloseTile"; id: TileId }
  | { kind: "Back" }
  | { kind: "Flagged"; i: number }
  | { kind: "Link"; i: number }
  | { kind: "Item"; i: number }
  | { kind: "Fix"; i: number }
  | { kind: "Send"; i: number };

export interface Recent {
  path: string;
  name: string;
  opened: string;
}

/** A repository saved on the start screen. */
export interface Saved {
  path: string;
  name: string;
}

/** A folder the add sheet was given: its repository, and the GitHub
 *  repository whose pull requests it lists (null without one). */
export interface RepoCheck {
  path: string;
  name: string;
  github: string | null;
}

/** A pull request's link: its repository (`owner/repo`) and number, and
 *  the clone it opens in (null when none is known yet). */
export interface PrLink {
  url: string;
  repo: string;
  number: number;
  clone: Saved | null;
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
  /** The buttons show (View › Show Buttons). */
  buttons: boolean;
}
