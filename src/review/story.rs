//! Stories: changed units that call each other, read together. Splitting a
//! change along definition and use (Barnett et al., ICSE 2015) and reading
//! related parts next to each other (Baum et al., ICSME 2017) is what helps
//! a reviewer; alphabetical order by file is what tools default to.
//!
//! A story is a connected part of the call graph between the given units.
//! It is read from its entry points (units no other unit of the story
//! calls) down through what they call, depth first, so a caller comes right
//! before the units it calls.

use std::collections::{HashMap, HashSet};

use super::model::Unit;

/// Where one unit goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    /// Which story, in reading order.
    pub story: usize,
    /// Reading position over all units placed.
    pub rank: usize,
    /// The unit it is read under (index into the units), if any.
    pub parent: Option<usize>,
}

fn place_key(units: &[Unit], i: usize) -> (&std::path::Path, u32) {
    (units[i].file.as_path(), units[i].display_line())
}

/// Place `members` (indices into `units`) in stories. Stories are ordered by
/// the first file and line among their units; inside one, callees follow
/// their caller in file and line order.
pub fn place(units: &[Unit], members: &[usize]) -> HashMap<usize, Placement> {
    let by_id: HashMap<&str, usize> = members.iter().map(|&i| (units[i].id.as_str(), i)).collect();
    let member: HashSet<usize> = members.iter().copied().collect();
    let mut callees: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut called: HashSet<usize> = HashSet::new();
    let mut linked: HashMap<usize, Vec<usize>> = HashMap::new();
    for &i in members {
        for id in units[i].callees.iter().chain(&units[i].callers) {
            let Some(&j) = by_id.get(id.as_str()) else {
                continue;
            };
            if j == i {
                continue;
            }
            linked.entry(i).or_default().push(j);
        }
        for id in &units[i].callees {
            if let Some(&j) = by_id.get(id.as_str())
                && j != i
            {
                callees.entry(i).or_default().push(j);
                called.insert(j);
            }
        }
    }
    for v in callees.values_mut() {
        v.sort_by(|&a, &b| place_key(units, a).cmp(&place_key(units, b)));
        v.dedup();
    }

    // Connected parts, ignoring direction.
    let mut sorted: Vec<usize> = members.to_vec();
    sorted.sort_by(|&a, &b| place_key(units, a).cmp(&place_key(units, b)));
    sorted.dedup();
    let mut part_of: HashMap<usize, usize> = HashMap::new();
    let mut parts: Vec<Vec<usize>> = Vec::new();
    for &start in &sorted {
        if part_of.contains_key(&start) {
            continue;
        }
        let n = parts.len();
        let mut part = Vec::new();
        let mut stack = vec![start];
        part_of.insert(start, n);
        while let Some(i) = stack.pop() {
            part.push(i);
            for &j in linked.get(&i).into_iter().flatten() {
                if member.contains(&j) && !part_of.contains_key(&j) {
                    part_of.insert(j, n);
                    stack.push(j);
                }
            }
        }
        part.sort_by(|&a, &b| place_key(units, a).cmp(&place_key(units, b)));
        parts.push(part);
    }

    // Read each part from its entry points; a cycle with no entry starts at
    // its first unit in file order.
    let mut out: HashMap<usize, Placement> = HashMap::new();
    let mut rank = 0;
    for (story, part) in parts.iter().enumerate() {
        let entries = part.iter().copied().filter(|i| !called.contains(i));
        let rest = part.iter().copied();
        for root in entries.chain(rest) {
            if out.contains_key(&root) {
                continue;
            }
            let mut stack = vec![(root, None)];
            while let Some((i, parent)) = stack.pop() {
                if out.contains_key(&i) {
                    continue;
                }
                out.insert(
                    i,
                    Placement {
                        story,
                        rank,
                        parent,
                    },
                );
                rank += 1;
                for &j in callees.get(&i).into_iter().flatten().rev() {
                    if !out.contains_key(&j) {
                        stack.push((j, Some(i)));
                    }
                }
            }
        }
    }
    out
}

/// How far to indent unit `i` under the units it is read beneath: the number
/// of its ancestors in the story that `same` accepts (the same list group).
pub fn depth(placed: &HashMap<usize, Placement>, i: usize, same: impl Fn(usize) -> bool) -> usize {
    let mut d = 0;
    let mut cur = placed.get(&i).and_then(|p| p.parent);
    let mut seen = HashSet::new();
    while let Some(p) = cur {
        if !seen.insert(p) {
            break;
        }
        if same(p) {
            d += 1;
        }
        cur = placed.get(&p).and_then(|x| x.parent);
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(id: &str, file: &str, line: u32, callees: &[&str], callers: &[&str]) -> Unit {
        serde_json::from_value(serde_json::json!({
            "id": id, "file": file, "symbol": id, "kind": "Function",
            "status": "Changed", "old_range": null,
            "new_range": {"start": line, "end": line + 1},
            "fingerprint": "x", "changes": [],
            "callers": callers, "callees": callees, "decision": "Pending"
        }))
        .unwrap()
    }

    fn order<'a>(units: &'a [Unit], members: &[usize]) -> Vec<&'a str> {
        let placed = place(units, members);
        let mut v: Vec<usize> = members.to_vec();
        v.sort_by_key(|i| placed[i].rank);
        v.iter().map(|&i| units[i].id.as_str()).collect()
    }

    #[test]
    fn a_caller_comes_right_before_what_it_calls() {
        // z.rs::handler calls a.rs::helper; b.rs::other is on its own.
        let units = vec![
            unit("helper", "a.rs", 1, &[], &["handler"]),
            unit("other", "b.rs", 1, &[], &[]),
            unit("handler", "z.rs", 1, &["helper"], &[]),
        ];
        assert_eq!(order(&units, &[0, 1, 2]), ["handler", "helper", "other"]);
        let placed = place(&units, &[0, 1, 2]);
        assert_eq!(placed[&0].parent, Some(2));
        assert_eq!(placed[&0].story, placed[&2].story);
        assert_ne!(placed[&1].story, placed[&2].story);
        assert_eq!(depth(&placed, 0, |_| true), 1);
        assert_eq!(depth(&placed, 0, |p| p != 2), 0);
    }

    #[test]
    fn depth_first_with_siblings_in_file_order() {
        let units = vec![
            unit("root", "m.rs", 1, &["b", "a"], &[]),
            unit("a", "a.rs", 1, &["leaf"], &["root"]),
            unit("b", "b.rs", 1, &[], &["root"]),
            unit("leaf", "z.rs", 9, &[], &["a"]),
        ];
        assert_eq!(order(&units, &[0, 1, 2, 3]), ["root", "a", "leaf", "b"]);
        let placed = place(&units, &[0, 1, 2, 3]);
        assert_eq!(depth(&placed, 3, |_| true), 2);
    }

    #[test]
    fn cycles_and_units_outside_the_members_are_handled() {
        // a and b call each other; c calls a but is not a member.
        let units = vec![
            unit("b", "b.rs", 1, &["a"], &["a"]),
            unit("a", "a.rs", 1, &["b"], &["b", "c"]),
            unit("c", "c.rs", 1, &["a"], &[]),
        ];
        assert_eq!(order(&units, &[0, 1]), ["a", "b"]);
        let placed = place(&units, &[0, 1]);
        assert_eq!(placed.len(), 2);
        assert_eq!(depth(&placed, 0, |_| true), 1);
    }

    #[test]
    fn an_entry_point_that_reaches_only_part_of_its_story() {
        // r calls y; p and q call each other and p calls y too.
        let units = vec![
            unit("r", "r.rs", 1, &["y"], &[]),
            unit("y", "y.rs", 1, &[], &["r", "p"]),
            unit("p", "p.rs", 1, &["q", "y"], &["q"]),
            unit("q", "q.rs", 1, &["p"], &["p"]),
        ];
        let got = order(&units, &[0, 1, 2, 3]);
        assert_eq!(got, ["r", "y", "p", "q"]);
        let placed = place(&units, &[0, 1, 2, 3]);
        assert!(placed.values().all(|p| p.story == 0));
    }
}
