//! The security lens's data: what teer finds in a change without an agent
//! ([`Fact`]), what the agent finds when asked ([`Audit`], [`Finding`]),
//! and how a finding was proven. Plain data, saved with the review.

use serde::{Deserialize, Serialize};

use crate::review::model::{Cite, Range, Risk, Side};

/// What a fact or a finding is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// An authorization or authentication check removed, weakened, or
    /// missing where its siblings have it.
    Authorization,
    /// A route, controller action, command, consumer or webhook: new, or
    /// with a new signature.
    EntryPoint,
    /// Untrusted input reaching a sink: SQL, shell, eval, deserialization,
    /// file paths, outgoing HTTP, templates, redirects.
    Injection,
    /// A credential in the code.
    Secret,
    /// Sensitive data leaving the process: logs, responses, events.
    Exposure,
    /// Packages added, bumped or locked differently; install scripts.
    SupplyChain,
    /// CI workflows and infrastructure as code.
    Ci,
    /// Security configuration: CORS, cookies, TLS verification, CSP, debug.
    Config,
    /// Weak randomness, hashing or comparison where a secret is involved.
    Crypto,
    /// Model output reaching a sink, tools handed to an agent, untrusted
    /// text in a prompt.
    Llm,
}

impl Class {
    pub const ALL: [Class; 10] = [
        Class::Authorization,
        Class::EntryPoint,
        Class::Injection,
        Class::Secret,
        Class::Exposure,
        Class::SupplyChain,
        Class::Ci,
        Class::Config,
        Class::Crypto,
        Class::Llm,
    ];

    /// Its name in the config and in saved reviews: `authorization`,
    /// `entry_point`, `supply_chain`…
    pub fn id(self) -> &'static str {
        match self {
            Class::Authorization => "authorization",
            Class::EntryPoint => "entry_point",
            Class::Injection => "injection",
            Class::Secret => "secret",
            Class::Exposure => "exposure",
            Class::SupplyChain => "supply_chain",
            Class::Ci => "ci",
            Class::Config => "config",
            Class::Crypto => "crypto",
            Class::Llm => "llm",
        }
    }

    pub fn from_id(id: &str) -> Option<Class> {
        Class::ALL.into_iter().find(|c| c.id() == id)
    }

    /// One short word, as the attack-surface card and "checked:" list it.
    pub fn word(self) -> &'static str {
        match self {
            Class::Authorization => "auth",
            Class::EntryPoint => "entry points",
            Class::Injection => "injection",
            Class::Secret => "secrets",
            Class::Exposure => "data exposure",
            Class::SupplyChain => "packages",
            Class::Ci => "ci",
            Class::Config => "config",
            Class::Crypto => "crypto",
            Class::Llm => "llm",
        }
    }
}

/// How much a fact weighs: the least level it puts its unit at.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Weight {
    /// Shown as a reason; the level stays.
    #[default]
    Note,
    /// At least worth a look (◆).
    Look,
    /// Needs a close look (▲).
    Attention,
}

/// Where a fact came from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "from", rename_all = "lowercase")]
pub enum Origin {
    /// teer's own reading of the code.
    #[default]
    Teer,
    /// Another tool's finding, read from SARIF or GitHub code scanning:
    /// its name (`CodeQL`, `Semgrep`) and the rule that fired.
    Import { tool: String, rule: String },
}

/// A security fact about a unit, found in the change without an agent (or
/// imported from another tool). Facts set levels: an agent never removes
/// one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fact {
    pub class: Class,
    pub weight: Weight,
    /// One short sentence, as "why this level" lists it:
    /// "denyAccessUnlessGranted is gone".
    pub text: String,
    /// The side its lines are on: a removed check is on the old side.
    pub side: Side,
    /// The lines in the unit's file that show it, when there are some.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<Range>,
    /// What it is about, by name: the check, the sink, the package, the
    /// route. Empty when there is no one name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub subject: String,
    #[serde(default, skip_serializing_if = "Origin::is_teer")]
    pub origin: Origin,
}

impl Origin {
    pub fn is_teer(&self) -> bool {
        *self == Origin::Teer
    }
}

/// The Security tile's answer after validation: the findings that kept
/// their evidence, the controls found in place, and what was checked.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Audit {
    /// Most severe first.
    pub findings: Vec<Finding>,
    /// Controls the agent checked and found in place.
    #[serde(default)]
    pub holds: Vec<Hold>,
    /// The classes the agent was asked to check.
    #[serde(default)]
    pub checked: Vec<Class>,
    /// What was left out on purpose, in words: "denial of service",
    /// "rate limits".
    #[serde(default)]
    pub not_checked: Vec<String>,
    /// Findings dropped for want of evidence (logged to `teer.log`).
    #[serde(default)]
    pub dropped: usize,
    /// The second model's check of the findings is on its way. Not saved.
    #[serde(skip)]
    pub checking: bool,
    /// The findings are being run old against new (`<space>p`). Not saved.
    #[serde(skip)]
    pub proving: bool,
}

/// One way the change can be abused, with the path that shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub class: Class,
    pub severity: Risk,
    pub confidence: Risk,
    /// One sentence: what goes wrong.
    pub title: String,
    /// Where the untrusted value enters (an entry point, a parameter).
    pub source: Cite,
    /// The calls between the source and the sink, in order.
    #[serde(default)]
    pub path: Vec<Hop>,
    /// Where it does harm.
    pub sink: Cite,
    /// The control that should be on the path, or why the input itself is
    /// the problem.
    pub missing: String,
    /// Who sends what, and what happens.
    pub exploit: String,
    /// Every hop is a call teer's graph knows about. A finding whose path
    /// isn't is shown, labelled "path not verified", and raises nothing.
    pub path_verified: bool,
    /// The second model's check of this finding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub second: Option<SecondCheck>,
    /// What running it old against new showed (`<space>p`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proof: Option<Proof>,
}

impl Finding {
    /// Does it count: its path is verified and the second model doesn't
    /// dispute it.
    pub fn stands(&self) -> bool {
        self.path_verified && self.second.as_ref().is_none_or(|s| s.agree)
    }
}

/// A call on a finding's path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hop {
    /// The unit or symbol, as teer names it: `AssetRepository::byId`.
    pub symbol: String,
    pub cite: Cite,
}

/// A control the agent checked and found in place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hold {
    /// One sentence: "the query binds its parameters".
    pub what: String,
    pub cite: Cite,
}

/// A second model's word on a finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecondCheck {
    pub agree: bool,
    /// One sentence.
    pub why: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Cite>,
}

/// What running a finding's attacker case old against new showed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Proof {
    /// The case does what the finding says with the change.
    Proven {
        case: String,
        before: String,
        after: String,
    },
    /// It ran, and didn't show it.
    NotShown {
        case: String,
        before: String,
        after: String,
    },
    /// It couldn't run in isolation (a database, a framework kernel), or
    /// the probe failed: why.
    Unproven { why: String },
}
