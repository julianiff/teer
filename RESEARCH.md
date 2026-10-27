# What helps you judge a change you can't read

The question behind teer: a code change in a codebase and a language you don't know, with visualizations and correct text beside it. What lets you make a confident and correct accept/reject decision? This file keeps what a sweep of about ten years of research (2015–2026, plus a few older classics) turned up, what is built in teer, and what is still open. Written 2026-10-16/17.

## The one rule

**A picture or a text helps only when it lets you check a claim without reading the code.** Explanations raise acceptance whether the AI is right or wrong; correct text describes a change but doesn't tell you it is wrong.

- Bansal et al., CHI 2021: AI help raised team accuracy, explanations added nothing on top. https://arxiv.org/abs/2006.14779
- Fok & Weld, AI Magazine 2024: explanations help only when they make the AI's answer verifiable. https://arxiv.org/abs/2305.07722
- Kaufman, Brun, Murali, Endres, 2026 preprint (86 Python developers): correct code judged right 74% of the time, wrong code 49% (chance), with the same confidence; weak explanations lowered accuracy and raised confidence. https://arxiv.org/abs/2607.08885
- Tufano et al., ICSE 2025 (29 reviewers): with AI review text, reviewers looked where it pointed; more minor issues found, no more serious ones, no time saved. https://arxiv.org/abs/2411.11401
- Buçinca et al., CSCW 2021: making people think before seeing the AI's answer reduces over-reliance. https://arxiv.org/abs/2102.09692

So every view should be a **receipt** (values, names, counts, lines that exist) rather than a story.

## What the research says works

| Evidence | What | Source |
|---|---|---|
| Strongest | Views of what code does when it runs. Extravis: 22% less time, 43% more correct answers; replicated with ExplorViz | Cornelissen et al., TSE 2011; Fittkau et al., ICPC 2015 |
| Controlled | Riskiest part first: a defect shown last had 64% lower odds of being found | Fregnan et al., FSE 2022, https://arxiv.org/abs/2208.04259 |
| Field + survey | Splitting a change by definition and use: 42% of Microsoft changes split cleanly | Barnett et al., ICSE 2015 (ClusterChanges) |
| Field | Folding refactorings: lines to review for moves 14.5 → 2 | Brito & Valente 2021 (RAID), https://arxiv.org/abs/2103.11453 |
| Controlled | Risk from history shown to reviewers improved effectiveness; risk without a reason was rejected (Meta: 7 positive, 48 negative) | Gherald, EMSE 2024; Meta DRS 2025/2026 |
| Mining | Reviewers need: who uses the changed code, why it changed, the code around it | Pascarella et al., CSCW 2018 |
| Weak / none | Whole-system maps (code cities, treemaps), edge bundling (suggests edges that don't exist), UML (no effect in 4 experiments), similarity maps, PR graph tools (feel helpful, find no more defects) | Merino et al. 2018 (62% of published visualizations never evaluated); Scanniello et al., TOSEM 2014 |
| Behavior diff | Generate tests, run them on old and new code, show the differences; Testora labels each difference intended or not against the PR text: 11 of 13 reported regressions confirmed, ~$0.003 per PR | BERT (2010); Pradel, Testora, https://arxiv.org/abs/2503.18597 |
| Language | Wrong assumptions carried over from a known language caused 276 of 450 cross-language questions | Shrestha et al., ICSE 2020 |

Ideas taken from data science:
- **Layered data diffs**: counts first, then the schema, then sample rows (Datafold).
- **Lineage pruning**: drop callers that never touch the changed field.
- **Draw the difference itself**: rather than putting two versions side by side (Gleicher 2011/2018).
- **Degree of interest**: a small graph around the change instead of the whole one (van Ham & Perer 2009).
- **Honest uncertainty**: grey for "unknown", counts instead of a 0.73 score (Correll 2018; Kale 2021).
- **Risk as a waterfall**: built only from checkable factors.

## What is built in teer

| Idea | In teer |
|---|---|
| Riskiest first, related parts together | The list goes by attention level, then by story: callers right before what they call (`review::story`) |
| Behavior table (Testora-style) | `t`: the agent writes a probe; teer runs it in a macOS sandbox on its own checkout before and after the change |
| The text checked against the runs | Each probe case names the statement it tests; a surprise means the text is wrong |
| Do the tests notice it? | Behavior runs the project's tests with the change and with only this unit put back |
| Which tests cover it, what they miss | `c` (Tests): teer finds the tests that reach the unit (no agent); `a` has the agent say which cover it, with lines that exist, and what dangerous cases nothing tests; Behavior then probes those cases |
| Disagreement as a warning | A second model (Sonnet) checks every description; a dispute, a missed change or a higher risk makes the unit worth a look and shows `⚠` |
| Expensive to undo | The agent's word with lines, or a migration folder: `$ expensive to undo` and at least worth a look |
| Evidence for every claim | Statements, pictures, gaps and verdicts must cite lines that exist, or they are dropped |

### Field notes (app-server, 2026-10-16)

- Probes on PHP failed at first because `vendor` was linked into the probe checkout as a symlink. Composer then loads the project's classes relative to the autoloader's real path, which is the user's checkout, and the sandbox refuses to read that. The fix keeps the packages linked but makes `vendor/composer`, `bin` and `autoload.php` real copies.
- After the fix, `activate` (a new method of a migration CLI command) ran 6 cases. The project's own test file passed 5/5 with the change and failed 4/5 with the method taken out.
- On the same unit, Tests found all 5 covering tests and 4 gaps. The worst gap was a stale `external_id` being updated after the flag flip, with a swallowed API error counted as migrated.
- The first agent attempt timed out after 5 minutes on a large repository. Behavior now gets 10 minutes, and the prompt tells the agent to read only what it needs.

## Open ideas, not built yet

- **Test cases that verify the statements on new code.** For a new unit nothing exists before the change, so comparing runs can't check its statements. A case could assert what its statement says inside the probe and print whether it held, so teer reads the verdict from the run.
- **Intent from the PR description**, turned into checkable claims (for example "only negative amounts change") and tested by the probe like the statements are.
- **Answer before you see the AI's verdict.** On high-risk units, ask "did behavior change? yes / no / unsure" first (Buçinca 2021).
- **Ask the owner a yes/no question.** Find the code's owner with `git blame` and turn a surprise into one closed question: "`discount(-5)` now raises. Intended?"
- **Make wrong decisions cheaper**, not confidence higher: for changes that are expensive to undo, ask for a feature flag, a test, or a staged rollout.
- **Let the computer find inputs.** Run random inputs on both versions (differential fuzzing) and show the ones whose outputs differ. Today only the agent picks inputs.
- **Invariant diff**: "before, the result was never negative; after, it can raise" (Daikon-style, mined from probe runs).
- **Call sequence before and after**, in execution order, from traces: added calls green, removed calls red (AppMap, TraceDiff).
- **Mark observed vs inferred** in pictures: dashed edges for what only the model says.
- **A "not checked" line** that says what nobody looked at, against the anchoring Tufano found.
- **Notes on language traps**: where this language behaves differently from what readers assume (truthiness, aliasing, overflow, async).
- **Seeded known-bad changes** to measure how well the reviewer's confidence matches reality (lineup protocol, Wickham 2010).

## Sources by theme

- Software visualization: Merino et al., JSS 2018; Chotisarn et al. 2020 (https://arxiv.org/abs/2003.00643); Wettel, Lanza & Robbes, ICSE 2011 (CodeCity); Okoe et al., TVCG 2018 (node-link vs matrix); Sillito et al., TSE 2008 (questions programmers ask).
- Reviewing changes: Bacchelli & Bird, ICSE 2013; Baum et al., ICSME 2017 and EMSE 2019; di Biase et al. 2019; Huang et al., TSE 2022 (salient class); Spadini et al., ICSE 2019 (test-first review); ReviewVis (JSS 2023).
- Comparison and uncertainty: Gleicher, TVCG 2018; Nothelfer & Franconeri, TVCG 2020; Fernandes et al., CHI 2018; Correll, Moritz & Heer, CHI 2018.
- Tools seen: CodeRabbit Change Stack (diagrams per group, only when needed), Greptile, Qodo, CodeSee, AppMap, DeepWiki (confident diagrams nobody checked lead to bogus bug reports).
