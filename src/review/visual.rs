//! Visualize: the picture an agent drew of a flagged unit, saved as one
//! self-contained HTML page (inline SVG, no scripts, no external requests).
//! Everything the agent wrote, and every name from the repository, is
//! HTML-escaped; a Content-Security-Policy blocks scripts and loads as a
//! second line of defence.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::PathBuf;

use unicode_width::UnicodeWidthStr;

use super::model::{Cite, Delta, Review, Unit, UnitKind, Visual, VisualEdge};
use super::validate::{Region, unit_regions};
use crate::units::extract::MODULE_SYMBOL;
use crate::util::{hash_hex, sanitize};

/// Where a Visualize or Tests answer may cite, where a picture's page goes,
/// and its heading.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub regions: Vec<Region>,
    pub path: PathBuf,
    pub title: String,
}

pub fn target(review: &Review, unit: &Unit) -> Target {
    Target {
        regions: scope(review, unit),
        path: page_path(&review.id, &unit.id),
        title: title(unit),
    }
}

/// The code a picture may cite: the unit's own lines, then every caller's and
/// callee's lines from the call graph.
pub fn scope(review: &Review, unit: &Unit) -> Vec<Region> {
    let mut out = review
        .file_of(unit)
        .map(|f| unit_regions(unit, f))
        .unwrap_or_default();
    let nodes: HashMap<&str, _> = review
        .graph
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), n))
        .collect();
    for (ids, role) in [(&unit.callers, "caller"), (&unit.callees, "callee")] {
        for n in ids.iter().filter_map(|id| nodes.get(id.as_str())) {
            let r = Region {
                file: n.file.clone(),
                range: n.range,
                role: format!("{role} {}", sanitize(&n.symbol)),
            };
            if !out.contains(&r) {
                out.push(r);
            }
        }
    }
    out
}

/// `[A-Za-z0-9._-]` only, runs of anything else as one `_`, no leading dot.
pub(crate) fn safe_name(s: &str, max: usize) -> String {
    let mut out = String::new();
    for c in s.chars() {
        let c = if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
            c
        } else {
            '_'
        };
        if !(c == '_' && out.ends_with('_')) {
            out.push(c);
        }
        if out.len() >= max {
            break;
        }
    }
    let out = out.trim_start_matches('.');
    if out.is_empty() {
        "unit".into()
    } else {
        out.into()
    }
}

/// File name of a unit's page. The hash keeps ids that clean up to the same
/// name (`a/b::c`, `a_b::c`) apart.
pub fn page_name(unit_id: &str) -> String {
    format!(
        "{}-{}.html",
        safe_name(unit_id, 80),
        &hash_hex(&[unit_id.as_bytes()])[..8]
    )
}

/// `<data>/visuals/<review>/<unit>.html`.
pub fn page_path(review_id: &str, unit_id: &str) -> PathBuf {
    super::store::data_dir()
        .join("visuals")
        .join(safe_name(review_id, 64))
        .join(page_name(unit_id))
}

/// The page heading: the unit's name and where it is.
pub fn title(unit: &Unit) -> String {
    let name = if unit.kind == UnitKind::Module || unit.symbol == MODULE_SYMBOL {
        "(top level)".to_string()
    } else {
        sanitize(&unit.symbol)
    };
    format!(
        "{name} · {}:{}",
        sanitize(&unit.file.to_string_lossy()),
        unit.display_line()
    )
}

fn node_label<'a>(v: &'a Visual, id: &'a str) -> &'a str {
    v.node(id).map_or(id, |n| n.label.as_str())
}

/// One edge as text: `dispatch ─SendMessage→ n8n queue`.
pub fn edge_line(v: &Visual, e: &VisualEdge) -> String {
    format!(
        "{} ─{}→ {}",
        node_label(v, &e.from),
        e.label,
        node_label(v, &e.to)
    )
}

/// Write the page to `v.path` (atomically, creating its directory).
pub fn write_page(v: &Visual, title: &str) -> anyhow::Result<()> {
    super::store::write_atomic(&v.path, render_html(v, title).as_bytes())
}

// ----------------------------------------------------------------- layout

const NODE_H: i32 = 40;
const ROW_GAP: i32 = 28;
const PAD: i32 = 24;
/// Room above the graph for a self-loop and its label.
const LOOP_ROOM: i32 = 44;
const MIN_GAP: i32 = 80;
const LABEL_H: i32 = 16;
/// How far an edge label sits off its line, so the line never runs through it.
const LABEL_LIFT: f64 = 10.0;

/// Estimated width of `s` in pixels, at `tenths` tenths of a pixel per column.
fn text_w(s: &str, tenths: i32) -> i32 {
    (s.width() as i32 * tenths + 9) / 10
}

fn node_w(label: &str) -> i32 {
    (text_w(label, 74) + 28).clamp(88, 340)
}

fn label_w(label: &str) -> i32 {
    text_w(label, 68) + 8
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Box2 {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

impl Box2 {
    fn overlaps(&self, o: &Box2) -> bool {
        self.x < o.x + o.w && o.x < self.x + self.w && self.y < o.y + o.h && o.y < self.y + self.h
    }
}

#[derive(Debug)]
struct EdgeShape {
    d: String,
    label_x: i32,
    label_y: i32,
}

#[derive(Debug)]
struct Layout {
    width: i32,
    height: i32,
    nodes: Vec<Box2>,
    /// One per edge of the picture; `None` for an edge whose end is unknown.
    edges: Vec<Option<EdgeShape>>,
}

/// Mark the edges that close a cycle (DFS back edges), so the rest is a DAG.
fn back_edges(n: usize, ends: &[Option<(usize, usize)>]) -> Vec<bool> {
    fn visit(u: usize, adj: &[Vec<(usize, usize)>], state: &mut [u8], back: &mut [bool]) {
        state[u] = 1;
        for &(ei, v) in &adj[u] {
            match state[v] {
                0 => visit(v, adj, state, back),
                1 => back[ei] = true,
                _ => {}
            }
        }
        state[u] = 2;
    }
    let mut adj = vec![Vec::new(); n];
    for (ei, e) in ends.iter().enumerate() {
        if let Some((a, b)) = *e
            && a != b
        {
            adj[a].push((ei, b));
        }
    }
    let mut state = vec![0u8; n];
    let mut back = vec![false; ends.len()];
    for u in 0..n {
        if state[u] == 0 {
            visit(u, &adj, &mut state, &mut back);
        }
    }
    back
}

/// Longest-path layering: each node one layer right of its furthest
/// predecessor along the edges that are not back edges.
fn layers(n: usize, forward: &[(usize, usize)]) -> Vec<usize> {
    let mut layer = vec![0usize; n];
    for _ in 0..n {
        for &(a, b) in forward {
            layer[b] = layer[b].max(layer[a] + 1);
        }
    }
    layer
}

/// The point at `t` on the cubic Bézier `p`.
fn bezier(p: [(f64, f64); 4], t: f64) -> (f64, f64) {
    let u = 1.0 - t;
    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    (
        a * p[0].0 + b * p[1].0 + c * p[2].0 + d * p[3].0,
        a * p[0].1 + b * p[1].1 + c * p[2].1 + d * p[3].1,
    )
}

fn layout(v: &Visual) -> Layout {
    let n = v.nodes.len();
    let idx: HashMap<&str, usize> = v
        .nodes
        .iter()
        .enumerate()
        .map(|(i, nd)| (nd.id.as_str(), i))
        .collect();
    let ends: Vec<Option<(usize, usize)>> = v
        .edges
        .iter()
        .map(|e| Some((*idx.get(e.from.as_str())?, *idx.get(e.to.as_str())?)))
        .collect();
    let back = back_edges(n, &ends);
    let forward: Vec<(usize, usize)> = ends
        .iter()
        .zip(&back)
        .filter_map(|(e, b)| e.filter(|(a, c)| a != c && !b))
        .collect();
    let layer = layers(n, &forward);
    let nl = layer.iter().max().map_or(0, |m| m + 1);
    let mut cols: Vec<Vec<usize>> = vec![Vec::new(); nl];
    for (i, l) in layer.iter().enumerate() {
        cols[*l].push(i);
    }

    let col_h = |len: usize| len as i32 * NODE_H + (len as i32 - 1).max(0) * ROW_GAP;
    let graph_h = col_h(cols.iter().map(Vec::len).max().unwrap_or(0));
    let self_loops = ends.iter().flatten().any(|(a, b)| a == b);
    let top = PAD + if self_loops { LOOP_ROOM } else { 0 };
    let centers = |cols: &[Vec<usize>]| -> Vec<i32> {
        let mut y = vec![0; n];
        for col in cols {
            let start = top + (graph_h - col_h(col.len())) / 2;
            for (k, &i) in col.iter().enumerate() {
                y[i] = start + k as i32 * (NODE_H + ROW_GAP) + NODE_H / 2;
            }
        }
        y
    };
    // Barycentre ordering: one sweep down by predecessors, one up by successors.
    for down in [true, false] {
        let order: Vec<usize> = if down {
            (1..nl).collect()
        } else {
            (0..nl.saturating_sub(1)).rev().collect()
        };
        for l in order {
            let y = centers(&cols);
            let mut keyed: Vec<(i64, usize, usize)> = cols[l]
                .iter()
                .enumerate()
                .map(|(k, &i)| {
                    let near: Vec<i64> = forward
                        .iter()
                        .filter_map(|&(a, b)| match down {
                            true => (b == i).then_some(y[a] as i64),
                            false => (a == i).then_some(y[b] as i64),
                        })
                        .collect();
                    let key = if near.is_empty() {
                        y[i] as i64 * 1000
                    } else {
                        near.iter().sum::<i64>() * 1000 / near.len() as i64
                    };
                    (key, k, i)
                })
                .collect();
            keyed.sort();
            cols[l] = keyed.into_iter().map(|t| t.2).collect();
        }
    }

    let col_w: Vec<i32> = cols
        .iter()
        .map(|c| {
            c.iter()
                .map(|&i| node_w(&v.nodes[i].label))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let gap: Vec<i32> = (0..nl)
        .map(|l| {
            v.edges
                .iter()
                .zip(&ends)
                .zip(&back)
                .filter(|((_, e), b)| !**b && e.is_some_and(|(a, c)| a != c && layer[a] == l))
                .map(|((edge, _), _)| label_w(&edge.label) + 32)
                .max()
                .unwrap_or(0)
                .max(MIN_GAP)
        })
        .collect();
    let mut col_x = vec![PAD; nl];
    for l in 1..nl {
        col_x[l] = col_x[l - 1] + col_w[l - 1] + gap[l - 1];
    }
    let width = if nl == 0 {
        2 * PAD
    } else {
        col_x[nl - 1] + col_w[nl - 1] + PAD
    };
    let y = centers(&cols);
    let nodes: Vec<Box2> = (0..n)
        .map(|i| {
            let l = layer[i];
            let w = node_w(&v.nodes[i].label);
            Box2 {
                x: col_x[l] + (col_w[l] - w) / 2,
                y: y[i] - NODE_H / 2,
                w,
                h: NODE_H,
            }
        })
        .collect();

    // Parallel edges between the same two nodes are spread apart.
    let mut count: HashMap<(usize, usize), i32> = HashMap::new();
    for e in ends.iter().flatten() {
        *count.entry(*e).or_default() += 1;
    }
    let mut seen: HashMap<(usize, usize), i32> = HashMap::new();
    let bottom = top + graph_h;
    let mut back_k = 0;
    let mut placed: Vec<Box2> = nodes.clone();
    let mut height = bottom + PAD;
    let mut edges = Vec::new();
    for (ei, e) in ends.iter().enumerate() {
        let Some((a, b)) = *e else {
            edges.push(None);
            continue;
        };
        let k = seen.entry((a, b)).or_default();
        let off = (2 * *k - (count[&(a, b)] - 1)) * 4;
        *k += 1;
        let (na, nb) = (nodes[a], nodes[b]);
        let (d, (lx, ly)) = if a == b {
            let cx = na.x + na.w / 2 + off;
            let t = na.y;
            (
                format!(
                    "M{},{t} C{},{} {},{} {},{}",
                    cx - 14,
                    cx - 14,
                    t - 36,
                    cx + 14,
                    t - 36,
                    cx + 14,
                    t - 1
                ),
                (cx as f64, (t - 34) as f64),
            )
        } else if back[ei] {
            let (sx, sb) = (na.x + na.w / 2 + off, na.y + na.h);
            let (tx, tb) = (nb.x + nb.w / 2 + off, nb.y + nb.h);
            let yb = bottom + 28 + back_k * 24;
            back_k += 1;
            height = height.max(yb + PAD);
            let p = [
                (sx as f64, sb as f64),
                (sx as f64, yb as f64),
                (tx as f64, yb as f64),
                (tx as f64, (tb + 1) as f64),
            ];
            (
                format!("M{sx},{sb} C{sx},{yb} {tx},{yb} {tx},{}", tb + 1),
                {
                    let (x, y) = bezier(p, 0.5);
                    (x, y + LABEL_LIFT)
                },
            )
        } else {
            let (x0, y0) = (na.x + na.w, na.y + na.h / 2 + off);
            let (x1, y1) = (nb.x - 1, nb.y + nb.h / 2 + off);
            let mx = (x0 + x1) / 2;
            let p = [
                (x0 as f64, y0 as f64),
                (mx as f64, y0 as f64),
                (mx as f64, y1 as f64),
                (x1 as f64, y1 as f64),
            ];
            // The label sits in the gap right after the source's column, so a
            // long edge never puts it on a node of a middle column.
            let la = layer[a];
            let want = (col_x[la] + col_w[la] + gap[la] / 2) as f64;
            let (mut lo, mut hi) = (0.0, 1.0);
            for _ in 0..30 {
                let mid = (lo + hi) / 2.0;
                if bezier(p, mid).0 < want {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            (format!("M{x0},{y0} C{mx},{y0} {mx},{y1} {x1},{y1}"), {
                let (x, y) = bezier(p, lo);
                (x, y - LABEL_LIFT)
            })
        };
        // Nudge the label down until it is clear of nodes and other labels.
        let lw = label_w(&v.edges[ei].label);
        let lx = (lx.round() as i32).clamp(lw / 2 + 4, (width - lw / 2 - 4).max(lw / 2 + 4));
        let mut ly = ly.round() as i32;
        for _ in 0..12 {
            let r = Box2 {
                x: lx - lw / 2,
                y: ly - LABEL_H / 2,
                w: lw,
                h: LABEL_H,
            };
            if !placed.iter().any(|p| p.overlaps(&r)) {
                break;
            }
            ly += 6;
        }
        placed.push(Box2 {
            x: lx - lw / 2,
            y: ly - LABEL_H / 2,
            w: lw,
            h: LABEL_H,
        });
        height = height.max(ly + LABEL_H / 2 + PAD);
        edges.push(Some(EdgeShape {
            d,
            label_x: lx,
            label_y: ly,
        }));
    }
    Layout {
        width,
        height,
        nodes,
        edges,
    }
}

// --------------------------------------------------------------- the page

/// Escape text for HTML element content and quoted attribute values.
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

fn cites(c: &[Cite]) -> String {
    c.iter()
        .map(|c| {
            if c.start == c.end {
                format!("{}:{}", c.file.display(), c.start)
            } else {
                format!("{}:{}-{}", c.file.display(), c.start, c.end)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

const STATUSES: [Delta; 4] = [
    Delta::Added,
    Delta::Changed,
    Delta::Removed,
    Delta::Unchanged,
];

/// The colours of one theme, shared by the page and the terminal picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub bg: &'static str,
    pub fg: &'static str,
    pub muted: &'static str,
    pub line: &'static str,
    pub node: &'static str,
    pub accent: &'static str,
    pub accent_bg: &'static str,
    pub removed: &'static str,
    pub changed: &'static str,
}

pub const LIGHT: Palette = Palette {
    bg: "#ffffff",
    fg: "#1f2328",
    muted: "#656d76",
    line: "#8c959f",
    node: "#f6f8fa",
    accent: "#0969da",
    accent_bg: "#ddf4ff",
    removed: "#cf222e",
    changed: "#9a6700",
};

pub const DARK: Palette = Palette {
    bg: "#0d1117",
    fg: "#e6edf3",
    muted: "#8d96a0",
    line: "#6e7681",
    node: "#161b22",
    accent: "#4493f8",
    accent_bg: "#0c2d6b",
    removed: "#f85149",
    changed: "#d29922",
};

/// A colour of the palette, by role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ink {
    Bg,
    Fg,
    Muted,
    Line,
    Node,
    Accent,
    AccentBg,
    Removed,
    Changed,
}

const INKS: [(Ink, &str); 9] = [
    (Ink::Bg, "bg"),
    (Ink::Fg, "fg"),
    (Ink::Muted, "muted"),
    (Ink::Line, "line"),
    (Ink::Node, "node"),
    (Ink::Accent, "accent"),
    (Ink::AccentBg, "accent-bg"),
    (Ink::Removed, "removed"),
    (Ink::Changed, "changed"),
];

impl Palette {
    fn ink(&self, i: Ink) -> &'static str {
        match i {
            Ink::Bg => self.bg,
            Ink::Fg => self.fg,
            Ink::Muted => self.muted,
            Ink::Line => self.line,
            Ink::Node => self.node,
            Ink::Accent => self.accent,
            Ink::AccentBg => self.accent_bg,
            Ink::Removed => self.removed,
            Ink::Changed => self.changed,
        }
    }

    /// `--bg: #…; --fg: #…; …` for the page's CSS.
    fn css_vars(&self) -> String {
        INKS.iter()
            .map(|(i, name)| format!("--{name}: {};", self.ink(*i)))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// How the SVG names its colours: CSS variables, so the page follows the
/// system's light or dark mode, or the fixed colours of one palette, for
/// turning it into pixels (resvg knows no CSS variables).
#[derive(Debug, Clone, Copy)]
pub enum Paint {
    Css,
    Fixed(&'static Palette),
}

impl Paint {
    fn ink(self, i: Ink) -> String {
        match self {
            Paint::Css => {
                let name = INKS.iter().find(|(k, _)| *k == i).map_or("fg", |(_, n)| n);
                format!("var(--{name})")
            }
            Paint::Fixed(p) => p.ink(i).to_string(),
        }
    }
}

/// Box style by status: fill (`None` for none), border, border width, dashes, text.
fn node_look(d: Delta) -> (Option<Ink>, Ink, &'static str, Option<&'static str>, Ink) {
    match d {
        Delta::Added => (Some(Ink::AccentBg), Ink::Accent, "1.8", None, Ink::Fg),
        Delta::Changed => (Some(Ink::Node), Ink::Changed, "3", None, Ink::Fg),
        Delta::Removed => (None, Ink::Removed, "1.2", Some("5 4"), Ink::Muted),
        Delta::Unchanged => (Some(Ink::Node), Ink::Line, "1.2", None, Ink::Fg),
    }
}

/// Arrow style by status: line, line width, dashes, label. A changed arrow is
/// outlined: a wide line in its colour under a thin one in the background's.
fn edge_look(d: Delta) -> (Ink, &'static str, Option<&'static str>, Ink) {
    match d {
        Delta::Added => (Ink::Accent, "2", None, Ink::Accent),
        Delta::Changed => (Ink::Changed, "6", None, Ink::Changed),
        Delta::Removed => (Ink::Removed, "1.4", Some("6 5"), Ink::Removed),
        Delta::Unchanged => (Ink::Line, "1.4", None, Ink::Fg),
    }
}

const FONT: &str = "-apple-system, BlinkMacSystemFont, 'Segoe UI', Helvetica, Arial, sans-serif";

const PAGE_CSS: &str = "\
body { margin: 0; background: var(--bg); color: var(--fg);
  font: 14px/1.5 -apple-system, BlinkMacSystemFont, \"Segoe UI\", Helvetica, Arial, sans-serif; }
main { max-width: 1200px; margin: 0 auto; padding: 24px 16px; }
h1 { font: 600 17px/1.4 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; margin: 0 0 4px; overflow-wrap: anywhere; }
.what { color: var(--muted); margin: 0 0 16px; }
figure { margin: 0; }
.scroll { overflow-x: auto; }
svg { display: block; }
figcaption { margin-top: 12px; font-size: 15px; }
.legend { list-style: none; padding: 0; margin: 16px 0 0; display: flex; flex-wrap: wrap; gap: 8px 20px; color: var(--muted); font-size: 13px; }
.legend i { display: inline-block; width: 22px; height: 12px; border-radius: 3px; vertical-align: -1px; margin-right: 6px;
  border: 1.2px solid var(--line); background: var(--node); }
.legend .added i { border: 1.8px solid var(--accent); background: var(--accent-bg); }
.legend .changed i { border: 3px solid var(--changed); }
.legend .removed i { border: 1.2px dashed var(--removed); background: none; }
.note { color: var(--muted); font-size: 12px; margin-top: 16px; }
";

/// The page's CSS: both palettes as variables, then the page styles.
fn css() -> String {
    format!(
        ":root {{\n  color-scheme: light dark;\n  {}\n}}\n@media (prefers-color-scheme: dark) {{\n  :root {{ {} }}\n}}\n{PAGE_CSS}",
        LIGHT.css_vars(),
        DARK.css_vars()
    )
}

fn legend_text(d: Delta) -> &'static str {
    match d {
        Delta::Added => "added by this change",
        Delta::Changed => "behaves differently",
        Delta::Removed => "removed by this change",
        Delta::Unchanged => "unchanged",
    }
}

/// The SVG of `v`, laid out left to right, every style inline.
pub fn svg(v: &Visual, paint: Paint) -> String {
    let l = layout(v);
    let ink = |i: Ink| paint.ink(i);
    let dash = |d: Option<&str>| {
        d.map(|d| format!(";stroke-dasharray:{d}"))
            .unwrap_or_default()
    };
    let mut s = String::new();
    let _ = writeln!(
        s,
        "<svg role=\"img\" aria-label=\"{}\" viewBox=\"0 0 {w} {h}\" width=\"{w}\" height=\"{h}\" style=\"font-family:{FONT};font-size:13px\">",
        esc(&v.claim),
        w = l.width,
        h = l.height
    );
    let _ = writeln!(
        s,
        "<rect width=\"{}\" height=\"{}\" style=\"fill:{}\"/>",
        l.width,
        l.height,
        ink(Ink::Bg)
    );
    s.push_str("<defs>\n");
    for d in STATUSES {
        let _ = writeln!(
            s,
            "  <marker id=\"ah-{}\" viewBox=\"0 0 10 10\" refX=\"9\" refY=\"5\" markerWidth=\"7\" markerHeight=\"7\" orient=\"auto\"><path d=\"M0,0 L10,5 L0,10 z\" style=\"fill:{}\"/></marker>",
            d.word(),
            ink(edge_look(d).0)
        );
    }
    s.push_str("</defs>\n");
    for (e, shape) in v.edges.iter().zip(&l.edges) {
        let Some(shape) = shape else { continue };
        let st = e.status.word();
        let (line, width, dashes, text) = edge_look(e.status);
        s.push_str("<g>\n");
        let _ = writeln!(
            s,
            "  <title>{} ({st}) · {}</title>",
            esc(&edge_line(v, e)),
            esc(&cites(&e.evidence))
        );
        let path = |style: String, marker: bool| {
            let m = if marker {
                format!(" marker-end=\"url(#ah-{st})\"")
            } else {
                String::new()
            };
            format!(
                "  <path d=\"{}\" style=\"fill:none;{style}\"{m}/>\n",
                shape.d
            )
        };
        if e.status == Delta::Changed {
            s.push_str(&path(
                format!("stroke:{};stroke-width:{width}", ink(line)),
                false,
            ));
            s.push_str(&path(
                format!("stroke:{};stroke-width:2", ink(Ink::Bg)),
                true,
            ));
        } else {
            s.push_str(&path(
                format!("stroke:{};stroke-width:{width}{}", ink(line), dash(dashes)),
                true,
            ));
        }
        let _ = writeln!(
            s,
            "  <text x=\"{}\" y=\"{}\" dy=\"0.35em\" text-anchor=\"middle\" style=\"font-size:12px;fill:{};stroke:{};stroke-width:4px;stroke-linejoin:round;paint-order:stroke\">{}</text>",
            shape.label_x,
            shape.label_y,
            ink(text),
            ink(Ink::Bg),
            esc(&e.label)
        );
        s.push_str("</g>\n");
    }
    for (nd, b) in v.nodes.iter().zip(&l.nodes) {
        let st = nd.status.word();
        let (fill, border, width, dashes, text) = node_look(nd.status);
        s.push_str("<g>\n");
        let _ = writeln!(
            s,
            "  <title>{} ({st}) · {}</title>",
            esc(&nd.label),
            esc(&cites(&nd.evidence))
        );
        let _ = writeln!(
            s,
            "  <rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"6\" style=\"fill:{};stroke:{};stroke-width:{width}{}\"/>",
            b.x,
            b.y,
            b.w,
            b.h,
            fill.map_or("none".to_string(), ink),
            ink(border),
            dash(dashes)
        );
        let _ = writeln!(
            s,
            "  <text x=\"{}\" y=\"{}\" dy=\"0.35em\" text-anchor=\"middle\" style=\"fill:{}\">{}</text>",
            b.x + b.w / 2,
            b.y + b.h / 2,
            ink(text),
            esc(&nd.label)
        );
        s.push_str("</g>\n");
    }
    s.push_str("</svg>\n");
    s
}

/// The whole page: heading, the picture with the claim as its caption, a
/// legend. `title` names the unit (from the repository, so escaped too).
pub fn render_html(v: &Visual, title: &str) -> String {
    let mut legend = String::new();
    for d in STATUSES {
        let _ = writeln!(
            legend,
            "  <li class=\"{}\"><i></i>{}</li>",
            d.word(),
            legend_text(d)
        );
    }
    format!(
        "<!doctype html>
<html lang=\"en\">
<head>
<meta charset=\"utf-8\">
<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'\">
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">
<meta name=\"color-scheme\" content=\"light dark\">
<title>{title} · teer</title>
<style>
{css}</style>
</head>
<body>
<main>
<h1>{title}</h1>
<p class=\"what\">What talks to this unit, and what the change adds or removes.</p>
<figure>
<div class=\"scroll\">
{svg}</div>
<figcaption>{claim}</figcaption>
</figure>
<ul class=\"legend\">
{legend}</ul>
<p class=\"note\">Drawn by a coding agent and checked by teer: every box and arrow cites lines in this unit, its callers or what it calls. Hover one to see them.</p>
</main>
</body>
</html>
",
        title = esc(title),
        css = css(),
        svg = svg(v, Paint::Css),
        claim = esc(&v.claim),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::VisualNode;

    fn cite(start: u32) -> Vec<Cite> {
        vec![Cite {
            file: "src/dispatch.py".into(),
            start,
            end: start + 2,
        }]
    }

    fn n(id: &str, label: &str, status: Delta) -> VisualNode {
        VisualNode {
            id: id.into(),
            label: label.into(),
            status,
            evidence: cite(10),
        }
    }

    fn e(from: &str, to: &str, label: &str, status: Delta) -> VisualEdge {
        VisualEdge {
            from: from.into(),
            to: to.into(),
            label: label.into(),
            status,
            evidence: cite(14),
        }
    }

    /// A dispatcher that moves from SNS to an SQS queue, with a callback cycle
    /// and a retry loop.
    fn fixture() -> Visual {
        Visual {
            claim: "dispatch now sends n8n actions to an SQS queue instead of SNS.".into(),
            nodes: vec![
                n("api", "POST /actions handler", Delta::Unchanged),
                n("dispatch", "dispatch", Delta::Changed),
                n("sns", "SNS topic", Delta::Removed),
                n("queue", "n8n SQS queue", Delta::Added),
                n("n8n", "n8n workflow", Delta::Unchanged),
                n("callback", "callback endpoint", Delta::Unchanged),
            ],
            edges: vec![
                e("api", "dispatch", "calls", Delta::Unchanged),
                e("dispatch", "sns", "Publish", Delta::Removed),
                e("dispatch", "queue", "SendMessage", Delta::Added),
                e("queue", "n8n", "triggers", Delta::Unchanged),
                e("n8n", "callback", "POST result", Delta::Changed),
                e("callback", "dispatch", "resumes", Delta::Unchanged),
                e("dispatch", "dispatch", "retries 3x", Delta::Added),
            ],
            path: "/data/visuals/r/dispatch.html".into(),
        }
    }

    #[test]
    fn golden_page_for_a_fixture_graph() {
        insta::assert_snapshot!(render_html(&fixture(), "dispatch · src/dispatch.py:10"));
    }

    #[test]
    fn layers_run_left_to_right_and_cycles_go_back() {
        let v = fixture();
        let l = layout(&v);
        let x = |id: &str| l.nodes[v.nodes.iter().position(|n| n.id == id).unwrap()].x;
        assert!(x("api") < x("dispatch"));
        assert!(x("dispatch") < x("queue") && x("dispatch") < x("sns"));
        assert!(x("queue") < x("n8n") && x("n8n") < x("callback"));
        // No two boxes overlap, and every edge has a shape.
        for (i, a) in l.nodes.iter().enumerate() {
            for b in &l.nodes[i + 1..] {
                assert!(!a.overlaps(b), "{a:?} {b:?}");
            }
        }
        assert!(l.edges.iter().all(Option::is_some));
        let ends: Vec<_> = v
            .edges
            .iter()
            .map(|e| {
                let p = |id: &str| v.nodes.iter().position(|n| n.id == id);
                Some((p(&e.from)?, p(&e.to)?))
            })
            .collect();
        let back = back_edges(v.nodes.len(), &ends);
        assert_eq!(back, [false, false, false, false, false, true, false]);
    }

    #[test]
    fn agent_text_is_escaped_and_nothing_is_loaded() {
        let mut v = fixture();
        v.claim = "Runs <script>alert(1)</script> & \"more\"".into();
        v.nodes[0].label = "<img src=x onerror=alert(1)>".into();
        v.edges[0].label = "a&b".into();
        let html = render_html(&v, "f<g> · a&b.py:1");
        assert!(!html.contains("<script"), "{html}");
        assert!(!html.contains("<img"), "{html}");
        assert!(html.contains("Runs &lt;script&gt;alert(1)&lt;/script&gt; &amp; &quot;more&quot;"));
        assert!(html.contains("aria-label=\"Runs &lt;script&gt;"));
        assert!(html.contains(">a&amp;b</text>"));
        assert!(html.contains("<h1>f&lt;g&gt; · a&amp;b.py:1</h1>"));
        // No scripts, links or loads; `url(#…)` only points inside the page.
        for bad in [
            "http:", "https:", "src=\"", "href=", "@import", "<link", "<iframe",
        ] {
            assert!(!html.contains(bad), "{bad}");
        }
        assert!(
            html.match_indices("url(")
                .all(|(i, _)| html[i..].starts_with("url(#"))
        );
        assert!(html.contains("default-src 'none'"));
        assert!(html.contains("prefers-color-scheme: dark"));
        assert!(html.contains("<svg role=\"img\""));
        assert!(html.contains("<figcaption>Runs &lt;script&gt;"));
    }

    #[test]
    fn fixed_paint_has_no_css_variables_and_uses_the_palette() {
        let v = fixture();
        let dark = svg(&v, Paint::Fixed(&DARK));
        assert!(!dark.contains("var("), "{dark}");
        assert!(!dark.contains("class="));
        for c in [DARK.bg, DARK.accent, DARK.removed, DARK.changed, DARK.line] {
            assert!(dark.contains(c), "{c}");
        }
        assert!(!dark.contains(LIGHT.accent));
        // Same shapes in both paints: only the colours differ.
        let css = svg(&v, Paint::Css);
        assert!(css.contains("stroke:var(--accent)"));
        assert_eq!(css.matches("<path").count(), dark.matches("<path").count());
        // The page carries both palettes, dark under the media query.
        let page = render_html(&v, "t");
        let dark_at = page.find("prefers-color-scheme: dark").unwrap();
        assert!(page.find(LIGHT.accent).unwrap() < dark_at);
        assert!(page.find(DARK.accent).unwrap() > dark_at);
    }

    #[test]
    fn page_names_are_safe_and_distinct() {
        assert!(page_name("src/a.py::f").starts_with("src_a.py_f-"));
        assert!(page_name("src/a.py::f").ends_with(".html"));
        assert_ne!(page_name("a/b::c"), page_name("a_b::c"));
        // One plain file name: no separator survives, no leading dot.
        let up = page_name("../../etc/passwd");
        assert!(
            up.starts_with("_.._etc_passwd-") && !up.contains('/'),
            "{up}"
        );
        assert!(page_name("..").starts_with("unit-"));
        assert!(page_name(&"x".repeat(500)).len() < 100);
        let p = page_path("r/../1", "a::b");
        assert_eq!(p.parent().unwrap().file_name().unwrap(), "r_.._1");
    }

    #[test]
    fn edge_lines_use_labels() {
        let v = fixture();
        assert_eq!(
            edge_line(&v, &v.edges[2]),
            "dispatch ─SendMessage→ n8n SQS queue"
        );
        let mut odd = v.edges[0].clone();
        odd.from = "ghost".into();
        assert_eq!(edge_line(&v, &odd), "ghost ─calls→ dispatch");
    }

    #[test]
    fn an_edge_to_an_unknown_node_is_left_out_of_the_picture() {
        let mut v = fixture();
        v.edges.push(e("dispatch", "ghost", "calls", Delta::Added));
        let html = render_html(&v, "t");
        assert!(!html.contains("ghost"));
        assert_eq!(html.matches("marker-end=").count(), 7);
    }
}
