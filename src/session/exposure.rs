//! The queue's security order (`zs`): by level, or by how exposed each
//! unit is. By exposure, the units with security facts or standing
//! findings come first, most exposed first (see
//! [`crate::security::order::exposure`]), in a box of their own above the
//! level boxes; the rest follow in the usual order. Listed by file, the
//! exposed units sit under their files' names in that box, the files by
//! their most exposed unit.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use super::app::{App, Grouping};
use crate::review::attention::Level;
use crate::security::SecurityConfig;
use crate::security::order::{Exposure, exposure};

/// The security lens on the queue.
#[derive(Debug, Clone, Default)]
pub struct SecurityLens {
    /// The lens is on (`security.toml`'s `enabled`, read once at start):
    /// the attack-surface card shows and `zs` works.
    pub enabled: bool,
    /// `zs`: the queue by exposure.
    pub by_exposure: bool,
    /// By exposure, how many units the security box at the top holds: the
    /// first this many positions of the queue's order. 0 by level.
    pub exposed: usize,
}

impl SecurityLens {
    /// The lens as `security.toml` sets it, listed by level.
    pub fn load() -> SecurityLens {
        SecurityLens {
            enabled: SecurityConfig::load().enabled,
            ..Default::default()
        }
    }
}

impl App {
    /// `zs`: the queue by level, or by how exposed each unit is. The cursor
    /// stays on its unit.
    pub(crate) fn toggle_security_order(&mut self) {
        if !self.security.enabled {
            self.status = "The security lens is off (enabled = false in security.toml).".into();
            return;
        }
        self.security.by_exposure = !self.security.by_exposure;
        self.rebuild_order();
        self.status = if !self.security.by_exposure {
            "Listed by level.".into()
        } else if self.security.exposed == 0 {
            "By exposure: no unit has security facts or findings, so by level.".into()
        } else {
            let n = self.security.exposed;
            format!(
                "By exposure: {n} unit{} with security facts or findings first.",
                if n == 1 { "" } else { "s" }
            )
        };
    }

    /// Is the unit at `pos` in the security box (listed by exposure)?
    pub fn in_security_box(&self, pos: usize) -> bool {
        pos < self.security.exposed
    }

    /// The most attention a unit in the security box needs: its title's
    /// colour.
    pub fn security_box_level(&self) -> Level {
        (0..self.security.exposed)
            .map(|p| self.shown_level(self.order[p]))
            .max()
            .unwrap_or(Level::Routine)
    }

    /// After the queue's usual order is made: by exposure, move the units
    /// with facts or standing findings to the top, most exposed first (the
    /// usual order among equals), and note how many. Listed by file, they
    /// stay together under their files, the files by their most exposed
    /// unit, and the rest's files go to the box of the most attention one
    /// of their remaining units needs. A story's indent means nothing in
    /// the security box: none.
    pub(crate) fn order_by_exposure(&mut self) {
        self.security.exposed = 0;
        if !self.security.by_exposure {
            return;
        }
        let units = &self.review.units;
        let score: HashMap<usize, Exposure> = self
            .order
            .iter()
            .map(|&i| (i, exposure(&units[i])))
            .collect();
        let (mut exposed, mut rest): (Vec<usize>, Vec<usize>) =
            self.order.iter().partition(|i| score[i].is_exposed());
        match self.grouping {
            Grouping::Unit => exposed.sort_by(|a, b| score[b].cmp(&score[a])),
            Grouping::File => {
                let mut most: HashMap<&PathBuf, Exposure> = HashMap::new();
                let mut first: HashMap<&PathBuf, usize> = HashMap::new();
                for (k, &i) in exposed.iter().enumerate() {
                    let m = most.entry(&units[i].file).or_default();
                    *m = (*m).max(score[&i]);
                    first.entry(&units[i].file).or_insert(k);
                }
                exposed.sort_by(|&a, &b| {
                    let (fa, fb) = (&units[a].file, &units[b].file);
                    most[fb]
                        .cmp(&most[fa])
                        .then_with(|| first[fa].cmp(&first[fb]))
                        .then_with(|| score[&b].cmp(&score[&a]))
                });
                // The files left in the level boxes, by what is left of them.
                let mut left: HashMap<&PathBuf, Level> = HashMap::new();
                for &i in &rest {
                    let l = left.entry(&units[i].file).or_insert(Level::Same);
                    *l = (*l).max(self.shown_level(i));
                }
                let fresh: HashSet<&PathBuf> = rest
                    .iter()
                    .filter(|&&i| self.changed.contains(&units[i].id))
                    .map(|&i| &units[i].file)
                    .collect();
                rest.sort_by(|&a, &b| {
                    let (fa, fb) = (&units[a].file, &units[b].file);
                    left[fb]
                        .cmp(&left[fa])
                        .then_with(|| fresh.contains(fb).cmp(&fresh.contains(fa)))
                        .then_with(|| fa.cmp(fb))
                });
            }
        }
        for &i in &exposed {
            if let Some(d) = self.depth.get_mut(i) {
                *d = 0;
            }
        }
        self.security.exposed = exposed.len();
        exposed.extend(rest);
        self.order = exposed;
    }
}
