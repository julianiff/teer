# Security review in teer: proposal

2026-11-10 · status: built on `feat/security-review` (phases 1–5); phase 6, `teer audit <path>`, not built

## In short

teer should not become a repo-wide vulnerability scanner. Anthropic, OpenAI, Google, GitHub and CodeRabbit all ship one, run on frontier models in hosted sandboxes. teer's place is **the change**, read by a person. Security becomes a lens over the review teer already does:

1. **facts first, no agent.** At prep time teer finds the security facts in the diff (an auth check removed, a new route, user input reaching a raw query, a secret, a new package, a CI trigger) and raises levels with them. The agent can raise a level, never lower it, so a comment in the code that says "this is safe" can't talk a unit down.
2. **evidence, not opinions.** The agent's findings have to name a path from an entry point to a sink, with every step a call teer's graph knows about and every line cited. If a finding can't show that, it's dropped. The second model checks every finding.
3. **prove it when it can.** The `<space>r` probe runner reruns a finding with an attacker's input, old code against new, in the sandbox.
4. **the person decides.** Findings sit inside the diff being read. Accept, flag or write a note. The notes go into the existing fix loop and the PR post.

What competitors don't have: the evidence sits in the diff the reviewer reads, PRs are treated as hostile by design, there's a call graph for cross-file authorization (where LLMs are weakest), and it runs locally with the user's own agent.

## Where it happens

| where | what | agent? |
| --- | --- | --- |
| **prep** | security facts per unit (see the next section). Each one becomes a reason under "why this level", and a few rules raise the level | no |
| **queue** | an "attack surface" card at the top: the entry points, sinks, removed checks, packages and CI the change touches. `zs` sorts the queue by exposure: units on a path from an entry point to a sink come first | no |
| **unit view** | `<space>x` opens a **Security** tile: the agent's findings with their paths, what holds, and "prove it" | yes |
| **flagged list / `:w`** | findings carry a severity. Notes with paths go to the fix agent. The PR post gets a Security section. SARIF export is optional | no |
| **later: import** | SARIF files and GitHub code-scanning alerts for the PR (`gh api …/code-scanning/alerts?ref=`) are pinned to their units as reasons. teer becomes where CodeQL, Semgrep or Claude Security findings get judged | no |
| **later: `teer audit <path>`** | review a folder as if all of it were new: the same units, levels, lens and decisions. A paced audit of one module, not a scan of everything | optional |

Why not a repo scanner: it's crowded, it needs hosted sandboxes and frontier models to be competitive, and its output is a list of findings for a security team to work through later. Researchers measured those lists at 14–18 % true positives with 2025 agents, and the same prompt found 3, then 6, then 11 issues (Semgrep). teer is good at turning a list into decisions, so it should take lists in, not add another one.

### Mockups

Queue, above the level boxes:

```
╭ attack surface ─────────────────────────────────────────────────────────╮
│ entry points  POST /automations (new) · AutomationController::update     │
│ sinks         raw SQL · outgoing HTTP · file write                       │
│ checks        ✗ denyAccessUnlessGranted gone from BrandController::export │
│ packages      guzzlehttp/psr7 2.7 → 2.8 (lock only, no manifest change)   │
│ ci            release.yml: pull_request_target added                     │
│ checked: auth · injection · secrets · packages · ci   not: dos · crypto  │
╰──────────────────────────────────────────────────────────────────────────╯
```

Unit view, `<space>x`:

```
╭ ␣x Security ────────────────────────────────────────────────────────╮
│ 1 finding · 2 checks hold                                             │
│                                                                       │
│ ! the asset id from the request reaches the query without the owner   │
│   check  (high · authorization)                                       │
│   route   AssetController::show            AssetController.php:41     │
│   → load  AssetRepository::byId            AssetRepository.php:88     │
│   ✗ isGranted('VIEW', $asset): the 6 other actions here call it       │
│     before loading                                                    │
│   second model agrees                                                 │
│   ␣p prove it: old and new with another tenant's asset id             │
│                                                                       │
│ ✓ the query binds its parameters           AssetRepository.php:90     │
│ ✓ the response leaves out the owner's email AssetController.php:52    │
╰─ agent (sonnet) ───────────────────────────── ␣p prove · tab next ───╯
```

## What it looks for

The list is ordered by what a review of a change catches better than a repo scan. Every class has a fact teer finds itself and a job for the agent.

| class | teer's fact (tree-sitter + graph, no agent) | the agent's job | level |
| --- | --- | --- | --- |
| **authorization regressions** | a check call (from the catalog: `isGranted`, `denyAccessUnlessGranted`, `#[IsGranted]`, `authorize`, `can`, middleware lists…) is in the old unit and gone from the new one. A new action lacks a check its siblings all have | is the check done somewhere else on the path (a voter, middleware, the caller)? Name it or the hole | ▲ removed from a unit still reachable, ◆ sibling gap |
| **new entry points** | a route, controller action, CLI command, queue consumer or webhook that is new or has a new signature | what does it trust from the caller, and who may call it? | ◆ |
| **input → sink** | a new call to a sink (raw SQL, shell or `exec`, `eval`, deserialization, file paths, outgoing HTTP, templates without escaping, redirects) in a unit reachable from an entry point | trace the value: tainted, sanitized or constant? | ◆ |
| **secrets and data exposure** | added lines that match known key prefixes or have high entropy. Logging or serializing fields named like `password`, `token`, `email` | is it real? Does it leave the process (log, response, event)? | ▲ secret, ◆ exposure |
| **supply chain** | package added or bumped, a lock changed without its manifest, install scripts, a git or URL source | is it known, maintained, a likely typosquat? | ◆ |
| **ci and infra** | workflow changes: `pull_request_target`, `permissions:` widened, actions not pinned to a sha, secrets passed to steps. IaC: public buckets, `0.0.0.0/0`, wildcard IAM | what can a fork PR now do? | ◆ |
| **security config** | CORS `*`, cookies losing `Secure`, `HttpOnly` or `SameSite`, TLS verification off, CSP loosened, debug on | does it reach production config? | ◆ |
| **crypto misuse** | non-crypto randomness for tokens, `md5`/`sha1` for passwords, `==` on secrets | is the value security-relevant? | ● raised by the agent only |
| **LLM surfaces** | model output passed to a sink (SQL, shell, file, HTTP), tools handed to an agent, untrusted text put into a system prompt | can untrusted text steer it? | ◆ |

**Left out by default**, as Anthropic's action does, to keep the noise down: denial of service, rate limits, resource exhaustion, generic "validate the input" with no impact shown, open redirects with no chain. Each can be turned on in the config.

**The catalog** (sinks, checks, entry points per language) is built in for the six split languages. The user adds the project's own names in `~/.config/teer/security.toml`, for example their voters or their query builder's raw methods. It is **never read from the tree under review**: a PR could otherwise switch off its own rules. For local changes, teer reads it from the base commit.

**Saying what was checked.** The card and the tile always list what was checked and what wasn't. "No finding" never reads as "safe".

## How a finding is built

The agent answers in a fixed shape, and teer validates it like statements today:

```
finding: class, severity (high|medium|low), confidence
  source:  cite (entry point or untrusted value)
  path:    [cite, …]   each hop must be an edge in teer's call graph
  sink:    cite
  missing: the control that should be on the path, or "none: the input is the problem"
  exploit: one sentence: who sends what, what happens
holds: [{what, cite}]  controls the agent checked and found in place
```

- **cite or drop**, as today. A path hop that isn't a graph edge marks the finding "path not verified". It shows, but can never raise the level.
- **the second model** gets each finding plus the cited code, and must agree or dispute with its own cite. A disputed finding shows `⚠`, the way disputed statements do.
- **the agent gets the path, not just the unit.** The cross-file context comes from the graph: callers up to the entry points, and the checks found along the way. That is the documented weak spot. Semgrep found LLM IDOR detection breaks down "when checks are split across files, hidden in middleware, or dependent on framework conventions".
- **prove it (`␣p`)** hands the finding to the probe runner. The agent writes the attacker's case (another tenant's id, `../`, a crafted payload) as a probe case, and teer runs it old against new in the sandbox. "With the change, tenant B's asset loads; before, 403" turns the finding into **proven**. Units that can't run in isolation (a database, a framework kernel) stay **unproven**, labelled as such.

## Competitive analysis

| tool | runs | scope | finds by | verifies by | output | who decides |
| --- | --- | --- | --- | --- | --- | --- |
| **Claude Code `/security-review` + GitHub Action** (Aug 2025) | locally, or on every PR | changed files | one agent pass over the diff, 10 categories | a separate false-positive filter pass (drops DoS, rate limits…) | PR line comments, JSON | whoever reads the comments. "Not hardened against prompt injection… only trusted PRs" |
| **Claude Security** (preview Feb 2026, public beta Apr 2026, Claude Code plugin beta) | on demand at claude.ai/security | repo | traces data flow rather than matching patterns | confidence, severity, impact and repro per finding | dashboard, patches through Claude Code | security team (Enterprise) |
| **OpenAI Codex Security** (Aardvark, Oct 2025 → Mar 2026) | continuously, on commits | repo + commits | reads code, writes and runs tests, uses tools | validates in an isolated environment | findings + patches | security team (Enterprise/Business/Edu) |
| **Google CodeMender** (research Oct 2025, preview Jul 2026) | Gemini Enterprise Agent Platform | repo | agent | builds and runs an exploit in a sandbox; an LLM judge checks the patch | patches | developer reviews the patch |
| **GitHub CodeQL + Copilot Autofix** (agentic autofix GA 15 Oct 2026) | CI | repo / PR | CodeQL rules | reruns CodeQL on the fix | alerts, draft PR | developer |
| **Semgrep Multimodal** (AI detection beta Mar 2026) | CI / PR | repo / PR | rules, with the engine as the LLM's tool; IDOR and logic | deterministic anchors | PR comments, can block a merge | developer / AppSec |
| **CodeRabbit Security** (13 Aug 2026) | PR + scheduled deep scan | both | map (entry points, trust boundaries, auth) → parallel hunters | separate verifier: reachable? protected elsewhere? evidence? | PR findings, fix PRs, SARIF | developer |
| **Cursor Bugbot + Agentic Security Review** (Mar 2026) | PR, daily automations | PR + repo | agents per segment | a classifier dedupes; the team triages in Slack; then a blocking gate | PR comment, Slack | security team |
| **DryRun Security** (DeepScan Feb 2026) | PR | PR (+ repo) | "contextual" analysis, policies in plain English | not public | PR comment | developer |
| **ZeroPath, Corgea** | PR + repo | both | AI-SAST, authz focus | vendor claims only | fix PRs | developer |
| **Socket** | PR | dependencies | package analysis: typosquats, install scripts | rules + AI alerts | PR comment, block | developer |

**Where they all converge.** Every tool is a bot that posts a list, and every one has added a verify stage: a filter pass, a second agent, an exploit run, or a rule engine as anchor. The industry agrees that raw LLM findings are too noisy. The 2025 numbers behind that: Claude Code 14 % and Codex 18 % true positives on 11 Python web apps, IDOR the best class at 22 %, injection the noisiest, and runs not repeatable (Semgrep). Neuro-symbolic work (IRIS, LLM + CodeQL) found 55 of 120 Java CVEs against CodeQL's 27, still with a high false-discovery rate. Models are better in 2026, but nobody's marketing claims the noise is gone.

**Gaps teer can own:**

1. **A person reads it.** None of them puts the evidence into the diff someone is reading and asks for a decision per unit that then goes to the fix agent. They hand over a queue of comments. teer is a queue of decisions.
2. **Hostile PRs.** Anthropic's action is for trusted PRs only. Lumia reported in Aug 2026 that Cursor's Bugbot and Security Review load project config, hooks included, from the reviewed branch. teer already uses its own checkout for PRs, loads no repo agent config, turns off git hooks, runs a sandbox with no network, and lets the agent only raise. Security review is where that posture matters most, so it belongs in the pitch.
3. **Authorization across files.** This is the documented LLM weak spot, and teer has the call graph to hand both the model and the person the path and the sibling checks.
4. **Local, bring your own model.** The big players' security products are enterprise-plan features that upload the code. teer runs on Claude Code, Codex or any API, including ollama for code that must not leave the machine.
5. **Aggregating findings.** Importing SARIF or code-scanning alerts makes teer complementary to CodeQL, Semgrep and Claude Security instead of a weaker rival.

**Where not to compete:** repo-wide hunting, fix PRs opened automatically, dashboards, compliance reports, merge gates.

## Risks

- **False confidence.** A lens can be read as coverage. Mitigation: "checked / not checked" is always visible, and the wording is "no finding", never "safe".
- **Catalog upkeep.** Six languages times their frameworks. Start with what the user reviews daily (PHP/Symfony, TypeScript), then grow from real misses.
- **Prompt injection in reviewed code.** The facts layer and "only raise" contain it. The agent's findings stay untrusted text, as everything the agent writes already is.
- **Probe limits.** "Prove it" works on macOS and for units that run alone. Most web handlers won't. Be honest in the UI: proven, unproven, path not verified.
- **Cost.** One security agent call per unit is too much. Run it on `<space>x` only, as the advice works now. Prep stays agent-free.

## Phases

1. **facts.** Catalogs for PHP, TypeScript/JavaScript and Go, then Rust and Python. Secrets, packages and CI workflows in every language. Security reasons and level rules, the attack-surface card, "checked / not checked". No agent, fully testable with fixtures.
2. **Security tile.** `<space>x` with the finding shape, path validation against the graph, the second model, the default exclusions.
3. **queue and finish.** `zs` exposure order, severity in the flagged list, findings in notes, the fix loop and the PR post, SARIF out.
4. **prove it.** The probe runner with attacker cases.
5. **import.** SARIF in, GitHub code-scanning alerts for the PR.
6. **maybe.** `teer audit <path>`.

## How to know it works

- **A benchmark of vulnerable diffs.** Reverse known CVE fix commits (CVEfixes-style) so each diff introduces the bug, across the six languages, plus a hand-made set of authorization cases in PHP. Also clean diffs, to measure noise.
- **Measure** true positives, false positives, and **stability over 3 runs**, per provider (Claude Code, Codex, API, ollama), with and without the graph path in the prompt. That last comparison is the claim to prove.
- **Dogfood** on real reviews: how many findings got flagged versus accepted, and how many security notes the fix loop resolved.

## Decided (2026-11-10)

- **Both uses** matter equally: your own agent-written changes (the fix loop) and other people's PRs (hostile-PR handling, the PR post).
- **Keys:** `<space>x` the Security tile, `<space>p` prove it, `zs` the queue's security order.
- **Languages first:** PHP, TypeScript (and JavaScript, which shares its catalog) and Go. Secrets, packages, CI and config are checked in every language.

## Sources

- Anthropic, claude-code-security-review (README: categories, false-positive filter, "trusted PRs only"): https://github.com/anthropics/claude-code-security-review
- Claude Security public beta: https://thenewstack.io/anthropics-claude-security-beta/ · https://www.infosecurity-magazine.com/news/anthropic-claude-security-for-ai/ · plugin: https://cybersecuritynews.com/anthropic-claude-security-plugin/
- OpenAI, Aardvark → Codex Security: https://openai.com/index/introducing-aardvark/
- Google CodeMender preview: https://www.helpnetsecurity.com/2026/07/24/google-codemender-ai-agent-code-security/ · https://cloud.google.com/blog/products/identity-security/find-and-fix-software-vulnerabilities-with-codemender
- GitHub agentic autofix GA: https://github.blog/changelog/2026-10-15-agentic-autofix-for-code-scanning-alerts-general-availability
- Semgrep Multimodal: https://semgrep.dev/blog/2026/attackers-cant-have-all-the-advantage-introducing-semgrep-multimodal · release notes: https://docs.semgrep.dev/release-notes/march-2026
- Semgrep, Claude Code and Codex on web apps (TPR, non-determinism): https://semgrep.dev/blog/2025/finding-vulnerabilities-in-modern-web-apps-using-claude-code-and-openai-codex · IDOR limits: https://app.semgrep.dev/blog/2025/can-llms-detect-idors-understanding-the-boundaries-of-ai-reasoning
- CodeRabbit Security: https://coderabbit.ai/blog/introducing-coderabbit-security
- Cursor security agents: https://cursor.com/blog/security-agents · Lumia on Bugbot: https://lumia.security/blog/cursor-bugbot-remote-code-execution
- DryRun DeepScan: https://securitybrief.co.uk/story/dryrun-raises-usd-8-7m-to-secure-ai-driven-coding
- ZeroPath vs Corgea: https://thectoclub.com/tools/corgea-vs-zeropath/
- Socket for GitHub: https://docs.socket.dev/docs/socket-for-github
- IRIS (LLM + CodeQL): https://arxiv.org/html/2405.17238v3
- AI review tools and merge blocking: https://blog.codacy.com/ai-code-review-tools-compared-2026-why-most-cant-safely-block-a-merge-and-what-code-governance-fixes
