//! The review data model. Everything here is plain data so `teer --json`
//! and the persistence layer can serialize it directly.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::git::{ChangedFile, SkippedFile};
use crate::graph::GraphData;
use crate::util::hash_hex;

/// Inclusive, 1-based line range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Range {
    pub start: u32,
    pub end: u32,
}

impl Range {
    pub fn new(start: u32, end: u32) -> Self {
        Range { start, end }
    }
    pub fn contains(&self, line: u32) -> bool {
        line >= self.start && line <= self.end
    }
    pub fn len(&self) -> u32 {
        self.end + 1 - self.start
    }
    pub fn is_empty(&self) -> bool {
        self.end < self.start
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRef {
    pub agent: String,
    pub id: String,
    /// Working directory the session was started in. Claude Code looks
    /// sessions up relative to it, so resumes must run from here.
    pub cwd: PathBuf,
}

impl SessionRef {
    /// No session context: a plain headless agent run in `cwd`.
    pub fn fresh(agent: &str, cwd: PathBuf) -> SessionRef {
        SessionRef {
            agent: agent.into(),
            id: String::new(),
            cwd,
        }
    }

    pub fn is_fresh(&self) -> bool {
        self.id.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Review {
    pub id: String,
    pub repo_root: PathBuf,
    /// Commit the changes are compared against.
    pub base: String,
    /// Commit being reviewed, or "WORKTREE" for uncommitted changes.
    pub head: String,
    /// The commit HEAD pointed at when the review was built. Used as the
    /// persistence key together with `base`.
    pub head_commit: String,
    pub agent_session: Option<SessionRef>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub files: Vec<ChangedFile>,
    pub units: Vec<Unit>,
    /// Changed files teer can't show (binary, not UTF-8, too large), so
    /// the review can say they were left out.
    #[serde(default)]
    pub skipped: Vec<SkippedFile>,
    #[serde(default)]
    pub graph: GraphData,
    /// Set when this reviews a pull request (`teer pr`): the review is then
    /// stored under the PR, not under the repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<PrInfo>,
    /// The commits between base and head and, for a pull request, its title
    /// and description. Empty for uncommitted changes.
    #[serde(default, skip_serializing_if = "Intent::is_empty")]
    pub intent: Intent,
    /// What the reviewer asked about the code (Ask), oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<Question>,
}

/// A question about the code, asked during the review, and the agent's
/// answer once it came.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    pub text: String,
    /// The unit on show when it was asked: what "this" means.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<Answer>,
    /// Why the agent couldn't answer. Not saved: a reloaded question
    /// without an answer can simply be asked again.
    #[serde(skip)]
    pub error: Option<String>,
}

/// The agent's answer to a [`Question`]: its text, the code it points at
/// (read by teer, never pasted by the agent), and the units of this change
/// it has to do with.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shown: Vec<Shown>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<Related>,
}

/// Lines of a file an answer shows, as teer read them when the answer came:
/// the reviewed version for a file of this change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shown {
    pub cite: Cite,
    /// What the lines are, in a few words.
    pub why: String,
    pub code: Vec<String>,
    /// The file is part of this change.
    #[serde(default)]
    pub in_change: bool,
    /// Lines of `cite` the change adds or edits.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<u32>,
}

/// A unit of this change an answer has to do with: named by the agent, or
/// holding code the answer shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Related {
    pub id: String,
    pub why: String,
}

/// What the author says the change is for: the commits between base and head
/// (oldest first) and, for a pull request, how GitHub describes it. All of
/// it is untrusted text, sanitised when it is read.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Intent {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commits: Vec<Commit>,
    /// The pull request's title, author, description and size (`gh pr view`),
    /// when gh answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<PrMeta>,
    /// Lines added and removed and files changed between base and head
    /// (`git diff --numstat`), for when gh didn't say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<DiffSize>,
    /// One or two sentences about the whole change, from the agent, once
    /// every unit is described.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

impl Intent {
    pub fn is_empty(&self) -> bool {
        self.commits.is_empty()
            && self.pr.is_none()
            && self.size.is_none()
            && self.summary.is_none()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffSize {
    pub additions: u32,
    pub deletions: u32,
    pub files: u32,
}

/// A pull request as GitHub describes it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrMeta {
    pub title: String,
    /// The author's login.
    pub author: String,
    /// The description, capped.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body: String,
    pub size: DiffSize,
}

/// One commit between base and head.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Commit {
    pub sha: String,
    pub short: String,
    pub author: String,
    /// `YYYY-MM-DD`.
    pub date: String,
    pub subject: String,
    /// The rest of the message, capped.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body: String,
    /// It has more than one parent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub merge: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<CommitFile>,
}

/// What one commit changed in one file, in line numbers of the reviewed
/// sides, so units can be matched to the commits that touched them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitFile {
    /// The path as the commit names it.
    pub path: PathBuf,
    /// The lines it added or changed, as lines of the reviewed head (later
    /// commits' shifts and renames followed). `None` when a later commit
    /// deleted the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<Lines>,
    /// The lines it removed or replaced, as lines of the base. `None` when
    /// the file did not exist at the base.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<Lines>,
}

/// Line ranges of one file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lines {
    pub path: PathBuf,
    pub ranges: Vec<Range>,
}

/// A pull request under review, as named by its URL.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrInfo {
    /// Lowercase host, without port.
    pub host: String,
    /// Port written in the URL, if any: another instance on the same host is another PR.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// The PR had no merge ref (conflicts?), so the base is the remote's
    /// default branch, named here. Shown in the header; not part of the identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_assumed: Option<String>,
    pub owner: String,
    pub repo: String,
    pub number: u32,
    /// The git remote the PR was fetched from (set by `teer pr`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// The PR head (full commit id) that is being reviewed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    /// The head the user approved on GitHub from teer. A new head starts unapproved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_head: Option<String>,
    /// The user's own checkout (not the PR worktree). Never persisted.
    #[serde(skip)]
    pub repo_dir: Option<PathBuf>,
}

impl PrInfo {
    /// This PR's reviewed head has been approved from teer.
    pub fn is_approved(&self) -> bool {
        self.head_sha.is_some() && self.head_sha == self.approved_head
    }

    /// Persistence key: the PR's repository and number, never a local path.
    pub fn store_key(&self) -> String {
        let repo = format!("{}/{}", self.owner, self.repo).to_ascii_lowercase();
        // Without a port the key is what earlier versions produced.
        let host = match self.port {
            Some(p) => format!("{}:{p}", self.host),
            None => self.host.clone(),
        };
        hash_hex(&[
            host.as_bytes(),
            repo.as_bytes(),
            format!("pr-{}", self.number).as_bytes(),
        ])
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnitKind {
    Function,
    Method,
    Class,
    Module,
    /// A whole file teer can't split into symbols (Markdown, JSON, a
    /// language it doesn't parse); see `units::whole`.
    File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnitStatus {
    Added,
    Removed,
    Changed,
    NoBehaviorChange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Decision {
    #[default]
    Pending,
    Accepted,
    /// Needs a closer look.
    Investigate,
}

/// The kinds of question about a flagged unit on the Flagged screen. "Dive"
/// names the agent side; the screen itself is `Screen::Flagged`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum DiveKind {
    /// What should be done about the change, ranked.
    Recommend,
    /// A picture of what talks to the unit and what the change adds or removes.
    Visualize,
    /// The unit run before and after the change on the same inputs, by a
    /// probe the agent wrote and teer ran in a sandbox.
    Behavior,
    /// A second model's check of the unit's description (`Unit::checks`).
    /// Only a tab: its answer lives in `Checks::second`, not in `Dive::results`.
    Disagree,
    /// Which existing tests cover the unit, and dangerous cases none does.
    Tests,
}

/// How a flagged unit was settled after looking into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Resolution {
    Ok,
    NeedsFix,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiveResult {
    pub text: String,
    pub sources: Vec<String>,
    /// The validated picture of a Visualize answer (`text` holds its claim).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visual: Option<Visual>,
    /// The table of a Behavior answer (`text` holds its summary).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behavior: Option<Behavior>,
    /// The covering tests and the gaps of a Tests answer (`text` holds its note).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<Coverage>,
}

/// An existing test that exercises the unit, at lines that exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoveringTest {
    /// The test's name, as the file has it.
    pub name: String,
    pub cite: Cite,
    /// What it checks, in one sentence.
    pub checks: String,
}

/// A case no test covers that could do harm, with the lines that make it so.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gap {
    /// The situation, in a few words: "the deactivation times out".
    pub case: String,
    /// What could go wrong then.
    pub danger: String,
    pub risk: Risk,
    pub evidence: Vec<Cite>,
}

/// How a test teer found reaches the unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "how")]
pub enum Reach {
    /// Calls it at `line`.
    Calls { line: u32 },
    /// Calls `via` at `line`, which reaches the unit in `hops` calls.
    Through { via: String, hops: u32, line: u32 },
    /// Names it (or, for a whole file, sits in a test file named after it)
    /// without a call teer could match.
    Names,
}

/// A test case teer's search of the test files found reaching the unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoundTest {
    /// As the file has it, with its class, module or `describe` in front.
    pub name: String,
    pub cite: Cite,
    pub reach: Reach,
}

/// What teer's search of the test files found, before any agent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestSearch {
    /// Nearest first, at most `test_search::MAX_FOUND`.
    pub tests: Vec<FoundTest>,
    /// How many it found before the cut.
    pub total: usize,
}

/// The Tests tile: the tests teer found, then, once asked, the agent's
/// reading of them: what covers the unit and what dangerous cases nothing does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coverage {
    /// `None` on answers from before teer searched by itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<TestSearch>,
    /// Whether the agent has read them: `covered` and `gaps` are its answer.
    /// Answers from before the search were the agent's alone.
    pub judged: bool,
    #[serde(default)]
    pub covered: Vec<CoveringTest>,
    #[serde(default)]
    pub gaps: Vec<Gap>,
}

impl Coverage {
    /// What the search found, before the agent has read it.
    pub fn found(search: TestSearch) -> Self {
        Coverage {
            search: Some(search),
            judged: false,
            covered: Vec::new(),
            gaps: Vec::new(),
        }
    }
}

/// What one run of the probe printed for a case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "kind", content = "text")]
pub enum Outcome {
    /// The value it returned, as compact JSON.
    Value(String),
    /// What it threw or failed with.
    Error(String),
    /// The run never printed this case.
    Missing,
}

/// Whether the agent said a case would come out the same after the change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Expect {
    Same,
    Different,
}

/// One input, run on the code before and after the change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeCase {
    pub name: String,
    /// The input, as the agent described it.
    pub input: String,
    /// The prediction, made before anything ran: what the statement it tests
    /// implies, or the agent's own view for a case that tests none.
    pub expect: Expect,
    pub why: String,
    pub before: Outcome,
    pub after: Outcome,
    /// The change statement (1-based, `Unit::changes`) this case tests; 0 for none.
    #[serde(default)]
    pub claim: usize,
}

impl ProbeCase {
    pub fn changed(&self) -> bool {
        self.before != self.after
    }

    /// The runs disagree with the agent's prediction.
    pub fn surprise(&self) -> bool {
        self.changed() != (self.expect == Expect::Different)
    }
}

/// What the project's own tests said about a unit: run with the change, and
/// again with only this unit put back the way it was.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "detail")]
pub enum TestCheck {
    /// They pass with the change and fail without it: they notice it.
    Guarded,
    /// They pass either way: no test notices this change.
    Unguarded,
    /// They already fail with the change applied (the run's status).
    FailWithChange(String),
    /// The agent found no test that exercises the unit (its reason).
    NoTests(String),
    /// Nothing ran (a removed unit, or the run couldn't start).
    NotRun(String),
}

/// A Behavior answer: every case, the command that ran, and where the probe
/// and its raw output were saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Behavior {
    pub cases: Vec<ProbeCase>,
    pub command: Vec<String>,
    pub dir: PathBuf,
    /// The statements the cases test, as they read when the probe ran.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub claims: Vec<String>,
    /// The project's tests with and without this unit's change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests: Option<TestCheck>,
    /// The command those tests ran with.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub test_command: Vec<String>,
}

/// How the runs bear on one statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimCheck {
    /// Every case testing it came out as the statement implies.
    Holds(usize),
    /// This many of its cases did not.
    Fails { wrong: usize, of: usize },
    /// No case tests it.
    Untested,
}

impl Behavior {
    /// Each statement in `claims` against the cases that test it.
    pub fn claim_checks(&self) -> Vec<(String, ClaimCheck)> {
        self.claims
            .iter()
            .enumerate()
            .map(|(i, text)| {
                let cases: Vec<&ProbeCase> =
                    self.cases.iter().filter(|c| c.claim == i + 1).collect();
                let wrong = cases.iter().filter(|c| c.surprise()).count();
                let check = match (cases.len(), wrong) {
                    (0, _) => ClaimCheck::Untested,
                    (n, 0) => ClaimCheck::Holds(n),
                    (n, w) => ClaimCheck::Fails { wrong: w, of: n },
                };
                (text.clone(), check)
            })
            .collect()
    }

    pub fn changed(&self) -> usize {
        self.cases.iter().filter(|c| c.changed()).count()
    }

    pub fn surprises(&self) -> usize {
        self.cases.iter().filter(|c| c.surprise()).count()
    }

    /// One sentence: how many cases ran, changed, and surprised.
    pub fn summary(&self) -> String {
        let n = self.cases.len();
        let s = if n == 1 { "" } else { "s" };
        let changed = match self.changed() {
            0 => "none behaves differently".to_string(),
            c if c == n => "all behave differently".to_string(),
            c => format!("{c} behave{} differently", if c == 1 { "s" } else { "" }),
        };
        let surprises = match self.surprises() {
            0 => "all as the agent predicted".to_string(),
            1 => "1 not as the agent predicted".to_string(),
            k => format!("{k} not as the agent predicted"),
        };
        format!("{n} case{s} ran before and after the change: {changed}, {surprises}.")
    }
}

/// What a change does to one part of a picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Delta {
    Added,
    Removed,
    Changed,
    Unchanged,
}

impl Delta {
    pub fn word(self) -> &'static str {
        match self {
            Delta::Added => "added",
            Delta::Removed => "removed",
            Delta::Changed => "changed",
            Delta::Unchanged => "unchanged",
        }
    }
}

/// Lines `start..=end` of `file` (relative to the repo root), cited by the agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cite {
    pub file: PathBuf,
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisualNode {
    pub id: String,
    pub label: String,
    pub status: Delta,
    pub evidence: Vec<Cite>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisualEdge {
    pub from: String,
    pub to: String,
    /// A short verb: "calls", "writes", "SendMessage".
    pub label: String,
    pub status: Delta,
    pub evidence: Vec<Cite>,
}

/// A Visualize answer after validation, and where its page was saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Visual {
    /// One sentence saying what the picture shows.
    pub claim: String,
    pub nodes: Vec<VisualNode>,
    pub edges: Vec<VisualEdge>,
    /// The HTML page in teer's data dir.
    pub path: PathBuf,
}

impl Visual {
    pub fn node(&self, id: &str) -> Option<&VisualNode> {
        self.nodes.iter().find(|n| n.id == id)
    }
}

/// Why undoing a unit after it ships would be costly. Only set when it is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    /// One sentence: "Rollback deletes the customers' workflows."
    pub why: String,
    /// The lines that show it.
    pub evidence: Vec<LineRef>,
}

/// A second model's verdict on one statement of the first description.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    /// The statement judged, as it read then.
    pub statement: String,
    pub agree: bool,
    pub why: String,
    pub evidence: Vec<LineRef>,
}

/// A second, independent model's check of the unit's description.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecondOpinion {
    /// Its own risk rating.
    pub risk: Risk,
    pub verdicts: Vec<Verdict>,
    /// Behavior changes the first description left out.
    pub missed: Vec<Change>,
}

impl SecondOpinion {
    /// Why it disagrees with `first` (the unit's statements), most important
    /// first; empty when it doesn't.
    pub fn disagreements(&self, first: &[Change]) -> Vec<String> {
        let mut out = Vec::new();
        for v in self.verdicts.iter().filter(|v| !v.agree) {
            out.push(format!(
                "disputes \"{}\": {}",
                crate::util::truncate(&v.statement, 48),
                v.why
            ));
        }
        for m in self.missed.iter().filter(|m| m.risk >= Risk::Medium) {
            out.push(format!("missed: {}", m.statement));
        }
        if let Some(had) = first.iter().map(|c| c.risk).max() {
            let word = |r: Risk| format!("{r:?}").to_lowercase();
            if self.risk > had || had as u8 >= self.risk as u8 + 2 {
                out.push(format!(
                    "rates it {} risk, not {}",
                    word(self.risk),
                    word(had)
                ));
            }
        }
        out
    }
}

/// A behavior change the commit messages and the PR description don't
/// mention. Only set when there is one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Unmentioned {
    /// One sentence: "Also stops sending the receipt email."
    pub text: String,
    /// The lines that show it.
    pub evidence: Vec<LineRef>,
}

/// What teer checked about a unit beyond its description.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Checks {
    /// Undoing it after it ships would be costly (the agent's word, with lines).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<Cost>,
    /// A change the author's commit messages and PR description leave out
    /// (the agent's word, with lines).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unmentioned: Option<Unmentioned>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub second: Option<SecondOpinion>,
    /// Why there is no second opinion, when asking for one failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub second_failed: Option<String>,
    /// A second opinion is on its way. Not persisted.
    #[serde(skip)]
    pub second_pending: bool,
}

impl Checks {
    pub fn is_empty(&self) -> bool {
        self.cost.is_none()
            && self.unmentioned.is_none()
            && self.second.is_none()
            && self.second_failed.is_none()
    }
}

/// Everything learned about a flagged unit on the Flagged screen.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dive {
    pub results: BTreeMap<DiveKind, DiveResult>,
    /// The questions in the order the reviewer asked them: the order of the
    /// Flagged screen's report.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub asked: Vec<DiveKind>,
    pub resolution: Option<Resolution>,
    /// The reviewer's note on the unit's whole change (`m`): for the agent,
    /// the clipboard, or the PR review.
    pub note: String,
    /// The unit's fingerprint when the note was written. The note stays when
    /// the code changes; this tells that it was about other code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note_on: Option<String>,
}

/// Where the agent-provided details for a unit stand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Prep {
    /// Not requested yet (large reviews prepare lazily).
    #[default]
    NotRequested,
    Pending,
    Ready,
    /// The agent failed, or every claim was dropped for lacking evidence.
    Unavailable,
    /// No agent call needed (no behavior change).
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Unit {
    /// Stable: file path + symbol path.
    pub id: String,
    pub file: PathBuf,
    pub symbol: String,
    pub kind: UnitKind,
    pub status: UnitStatus,
    pub old_range: Option<Range>,
    pub new_range: Option<Range>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renamed_from: Option<String>,
    /// Hash of the old and new source of the unit; decisions carry over
    /// between review runs while this stays the same.
    pub fingerprint: String,
    pub changes: Vec<Change>,
    #[serde(default)]
    pub prep: Prep,
    pub callers: Vec<String>,
    pub callees: Vec<String>,
    pub decision: Decision,
    /// One broad sentence about the whole unit, from the agent. Shown for
    /// routine units instead of their change statements.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Why a unit has no behavior change, found from the code (no agent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nbc_reason: Option<String>,
    /// Flagged-screen answers, resolution and note for a flagged unit.
    #[serde(default)]
    pub dive: Dive,
    /// Dive questions currently in flight. Not persisted.
    #[serde(skip)]
    pub dive_pending: BTreeSet<DiveKind>,
    /// Expensive to undo, and the second opinion.
    #[serde(default, skip_serializing_if = "Checks::is_empty")]
    pub checks: Checks,
}

impl Unit {
    pub fn needs_agent(&self) -> bool {
        self.status != UnitStatus::NoBehaviorChange
    }

    pub fn display_line(&self) -> u32 {
        self.new_range
            .or(self.old_range)
            .map(|r| r.start)
            .unwrap_or(0)
    }

    /// The note was written on code that has changed since.
    pub fn note_is_stale(&self) -> bool {
        !self.dive.note.trim().is_empty()
            && self
                .dive
                .note_on
                .as_ref()
                .is_some_and(|f| *f != self.fingerprint)
    }

    pub fn short_name(&self) -> &str {
        self.symbol.rsplit("::").next().unwrap_or(&self.symbol)
    }

    pub fn highest_risk(&self) -> Option<Risk> {
        self.changes.iter().map(|c| c.risk).max()
    }

    pub fn status_word(&self) -> &'static str {
        match self.status {
            UnitStatus::Added => "added",
            UnitStatus::Removed => "removed",
            UnitStatus::Changed => "changed",
            UnitStatus::NoBehaviorChange => "no behavior change",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Marker {
    Added,
    Changed,
    Removed,
}

impl Marker {
    pub fn symbol(self) -> &'static str {
        match self {
            Marker::Added => "+",
            Marker::Changed => "~",
            Marker::Removed => "−",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Old,
    New,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineRef {
    pub side: Side,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub marker: Marker,
    pub statement: String,
    pub evidence: Vec<LineRef>,
    pub risk: Risk,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_decisions_migrate_to_investigate() {
        for (old, want) in [
            ("\"Investigate\"", Decision::Investigate),
            ("\"Accepted\"", Decision::Accepted),
            ("\"Pending\"", Decision::Pending),
        ] {
            assert_eq!(
                serde_json::from_str::<Decision>(old).unwrap(),
                want,
                "{old}"
            );
        }
        assert_eq!(
            serde_json::to_string(&Decision::Investigate).unwrap(),
            "\"Investigate\""
        );
    }

    #[test]
    fn old_unit_json_loads_without_dive_fields() {
        let u: Unit = serde_json::from_value(serde_json::json!({
            "id": "a::f", "file": "a.rs", "symbol": "f", "kind": "Function",
            "status": "Changed", "old_range": null, "new_range": null,
            "fingerprint": "x", "changes": [], "callers": [], "callees": [],
            "decision": "Investigate"
        }))
        .unwrap();
        assert_eq!(u.dive, Dive::default());
        assert!(u.dive_pending.is_empty());
    }

    #[test]
    fn dive_round_trips_but_pending_is_not_persisted() {
        let mut u: Unit = serde_json::from_value(serde_json::json!({
            "id": "a::f", "file": "a.rs", "symbol": "f", "kind": "Function",
            "status": "Changed", "old_range": null, "new_range": null,
            "fingerprint": "x", "changes": [], "callers": [], "callees": [],
            "decision": "Investigate"
        }))
        .unwrap();
        u.dive.results.insert(
            DiveKind::Recommend,
            DiveResult {
                text: "t".into(),
                sources: vec!["s".into()],
                visual: None,
                behavior: None,
                coverage: None,
            },
        );
        u.dive.resolution = Some(Resolution::NeedsFix);
        u.dive.note = "n".into();
        u.dive_pending.insert(DiveKind::Visualize);
        let back: Unit = serde_json::from_str(&serde_json::to_string(&u).unwrap()).unwrap();
        assert_eq!(back.dive, u.dive);
        assert!(back.dive_pending.is_empty());
    }

    #[test]
    fn old_results_load_without_a_visual_and_visuals_round_trip() {
        let old: DiveResult =
            serde_json::from_value(serde_json::json!({"text": "t", "sources": []})).unwrap();
        assert_eq!(old.visual, None);
        assert!(!serde_json::to_string(&old).unwrap().contains("visual"));
        let cite = Cite {
            file: "a.rs".into(),
            start: 1,
            end: 2,
        };
        let v = DiveResult {
            text: "claim".into(),
            sources: Vec::new(),
            visual: Some(Visual {
                claim: "claim".into(),
                nodes: vec![VisualNode {
                    id: "a".into(),
                    label: "A".into(),
                    status: Delta::Added,
                    evidence: vec![cite.clone()],
                }],
                edges: vec![VisualEdge {
                    from: "a".into(),
                    to: "a".into(),
                    label: "calls".into(),
                    status: Delta::Unchanged,
                    evidence: vec![cite],
                }],
                path: "/d/visuals/r/a.html".into(),
            }),
            behavior: None,
            coverage: None,
        };
        let json = serde_json::to_string(&v).unwrap();
        assert!(json.contains("\"status\":\"added\""), "{json}");
        assert_eq!(serde_json::from_str::<DiveResult>(&json).unwrap(), v);
        assert_eq!(
            serde_json::from_str::<DiveKind>("\"Visualize\"").unwrap(),
            DiveKind::Visualize
        );
    }
}
