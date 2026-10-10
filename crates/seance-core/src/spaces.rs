//! Spaces: the rail's top tier. A space is a named bucket of circles the human
//! arranges; every circle sits in exactly one.
//!
//! Lives inside the rail-arrangement blob (`subscriptions.json`, key
//! `spaces`) that the daemon stores opaquely and every client edits — native
//! (`src/subscriptions_pref.rs`) and web (`seance-web/src/subs.rs`) both embed
//! [`Spaces`], so a client that drops the field on save wipes everyone's
//! spaces. The daemon's ctl verbs edit the same field in place.
//!
//! Rules (decided 2026-10-10):
//! - One space is the **default** (`general` on first run). It can be renamed,
//!   never deleted; it catches everything without a better home.
//! - A circle's label prefix (text before the first `-`) routes it until the
//!   human moves it by hand (`placed`). A space whose `prefix` matches catches
//!   it; otherwise a prefix shared by ≥2 unplaced circles mints an **auto**
//!   space. Generated names (`claude-49`) never count as a prefix.
//! - An auto space disappears when it empties; renaming it makes it permanent
//!   and keeps its prefix as an alias.
//! - No other space is special.
//!
//! Every step is deterministic in its inputs (sorted, ids derived from
//! prefixes) because two windows may reconcile the same new circle at once.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

pub const DEFAULT_ID: &str = "general";
pub const DEFAULT_NAME: &str = "general";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Space {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub auto: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spaces {
    /// Rail order.
    #[serde(default)]
    pub list: Vec<Space>,
    #[serde(default)]
    pub default: String,
    /// Circle slug → space id.
    #[serde(default)]
    pub of: BTreeMap<String, String>,
    /// Circles the human put somewhere by hand; prefixes no longer move them.
    #[serde(default)]
    pub placed: BTreeSet<String>,
}

/// The routing prefix of a label, lowercased. `None` without a hyphen, and for
/// generated names (`claude-49`, `codex-3`: a word and a number), which would
/// otherwise mint a space out of every unnamed agent.
pub fn space_prefix(label: &str) -> Option<String> {
    let p = crate::grouping::prefix_of(label)?;
    let (_, tail) = label.split_once('-')?;
    let tail = tail.trim();
    if !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(p)
}

impl Spaces {
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn get(&self, id: &str) -> Option<&Space> {
        self.list.iter().find(|s| s.id == id)
    }

    fn has(&self, id: &str) -> bool {
        self.get(id).is_some()
    }

    pub fn default_id(&self) -> &str {
        if self.has(&self.default) {
            &self.default
        } else {
            self.list
                .first()
                .map(|s| s.id.as_str())
                .unwrap_or(DEFAULT_ID)
        }
    }

    /// The space a circle renders in. Unassigned (not reconciled yet) reads as
    /// the default so no circle is ever invisible.
    pub fn space_of(&self, circle: &str) -> &str {
        match self.of.get(circle) {
            Some(id) if self.has(id) => id,
            _ => self.default_id(),
        }
    }

    /// `ordered` filtered to one space, order preserved.
    pub fn circles_in(&self, id: &str, ordered: &[String]) -> Vec<String> {
        ordered
            .iter()
            .filter(|c| self.space_of(c) == id)
            .cloned()
            .collect()
    }

    /// Exact id, else a unique case-insensitive name.
    pub fn resolve(&self, key: &str) -> Option<String> {
        if self.has(key) {
            return Some(key.to_string());
        }
        let mut hits = self
            .list
            .iter()
            .filter(|s| s.name.eq_ignore_ascii_case(key));
        let first = hits.next()?;
        if hits.next().is_some() {
            return None;
        }
        Some(first.id.clone())
    }

    fn fresh_id(&self, base: &str) -> String {
        let base = crate::util::slugify(base);
        if !self.has(&base) {
            return base;
        }
        (2..)
            .map(|n| format!("{base}-{n}"))
            .find(|id| !self.has(id))
            .expect("unbounded")
    }

    /// Bring the model in line with the live circles. `circles` is
    /// (slug, label); `focused` is the space the human is looking at, where a
    /// brand-new circle with no prefix home lands. Returns whether it changed.
    pub fn reconcile(&mut self, circles: &[(String, String)], focused: Option<&str>) -> bool {
        let before = self.clone();
        let mut circles: Vec<(String, String)> = circles.to_vec();
        circles.sort();

        if self.list.is_empty() {
            self.migrate(&circles);
        }
        if !self.has(&self.default) {
            if !self.has(DEFAULT_ID) {
                self.list.insert(
                    0,
                    Space {
                        id: DEFAULT_ID.into(),
                        name: DEFAULT_NAME.into(),
                        prefix: None,
                        auto: false,
                    },
                );
            }
            self.default = DEFAULT_ID.into();
        }

        let live: BTreeSet<&str> = circles.iter().map(|(s, _)| s.as_str()).collect();
        self.of.retain(|c, _| live.contains(c.as_str()));
        self.placed.retain(|c| live.contains(c.as_str()));

        let mut unplaced_count: BTreeMap<String, usize> = BTreeMap::new();
        for (slug, label) in &circles {
            if !self.placed.contains(slug) {
                if let Some(p) = space_prefix(label) {
                    *unplaced_count.entry(p).or_default() += 1;
                }
            }
        }

        let focused = focused.filter(|f| self.has(f)).map(str::to_string);
        for (slug, label) in &circles {
            let current = self.of.get(slug).filter(|id| self.has(id)).cloned();
            if self.placed.contains(slug) {
                if current.is_none() {
                    let d = self.default_id().to_string();
                    self.of.insert(slug.clone(), d);
                }
                continue;
            }
            let prefix = space_prefix(label);
            let home = match prefix.as_deref() {
                Some(p) => match self.list.iter().find(|s| s.prefix.as_deref() == Some(p)) {
                    Some(s) => Some(s.id.clone()),
                    None if unplaced_count.get(p).copied().unwrap_or(0) >= 2 => {
                        Some(self.mint_auto(p))
                    }
                    None => None,
                },
                None => None,
            };
            let target = match (home, current) {
                (Some(h), _) => h,
                // Renamed out of the prefix its auto space was built on.
                (None, Some(cur))
                    if self.get(&cur).is_some_and(|s| s.auto && s.prefix != prefix) =>
                {
                    self.default_id().to_string()
                }
                (None, Some(cur)) => cur,
                (None, None) => focused
                    .clone()
                    .unwrap_or_else(|| self.default_id().to_string()),
            };
            self.of.insert(slug.clone(), target);
        }

        let used: BTreeSet<String> = self.of.values().cloned().collect();
        self.list.retain(|s| !s.auto || used.contains(&s.id));
        *self != before
    }

    /// First run: the default space plus one auto space per prefix ≥2 circles
    /// share (the clusters the rail already drew). Nothing else is guessed.
    fn migrate(&mut self, circles: &[(String, String)]) {
        self.list.push(Space {
            id: DEFAULT_ID.into(),
            name: DEFAULT_NAME.into(),
            prefix: None,
            auto: false,
        });
        self.default = DEFAULT_ID.into();
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for (_, label) in circles {
            if let Some(p) = space_prefix(label) {
                *counts.entry(p).or_default() += 1;
            }
        }
        for (p, n) in counts {
            if n >= 2 {
                self.mint_auto(&p);
            }
        }
    }

    fn mint_auto(&mut self, prefix: &str) -> String {
        if let Some(s) = self
            .list
            .iter()
            .find(|s| s.prefix.as_deref() == Some(prefix))
        {
            return s.id.clone();
        }
        let id = self.fresh_id(prefix);
        self.list.push(Space {
            id: id.clone(),
            name: prefix.to_string(),
            prefix: Some(prefix.to_string()),
            auto: true,
        });
        id
    }

    pub fn create(&mut self, name: &str) -> String {
        let id = self.fresh_id(name);
        self.list.push(Space {
            id: id.clone(),
            name: name.trim().to_string(),
            prefix: None,
            auto: false,
        });
        id
    }

    /// Renaming makes an auto space permanent; its prefix stays as an alias.
    pub fn rename(&mut self, id: &str, name: &str) -> bool {
        let name = name.trim();
        match self.list.iter_mut().find(|s| s.id == id) {
            Some(s) if !name.is_empty() && (s.name != name || s.auto) => {
                s.name = name.to_string();
                s.auto = false;
                true
            }
            _ => false,
        }
    }

    /// The default can't be deleted. Its circles move to the default and stay
    /// there (placed), so a prefix doesn't immediately carry them off again.
    pub fn delete(&mut self, id: &str) -> bool {
        if id == self.default_id() || !self.has(id) {
            return false;
        }
        let d = self.default_id().to_string();
        for (c, s) in self.of.iter_mut() {
            if s == id {
                *s = d.clone();
                self.placed.insert(c.clone());
            }
        }
        self.list.retain(|s| s.id != id);
        true
    }

    /// A move by hand: sticky from here on.
    pub fn move_circle(&mut self, circle: &str, id: &str) -> bool {
        if !self.has(id) {
            return false;
        }
        let changed =
            self.of.get(circle).map(String::as_str) != Some(id) || !self.placed.contains(circle);
        self.of.insert(circle.to_string(), id.to_string());
        self.placed.insert(circle.to_string());
        changed
    }

    /// Move space `id` to sit before `before` (end when `None`).
    pub fn reorder(&mut self, id: &str, before: Option<&str>) -> bool {
        let Some(from) = self.list.iter().position(|s| s.id == id) else {
            return false;
        };
        if before == Some(id) {
            return false;
        }
        let snapshot = self.list.clone();
        let s = self.list.remove(from);
        let to = before
            .and_then(|b| self.list.iter().position(|x| x.id == b))
            .unwrap_or(self.list.len());
        self.list.insert(to, s);
        self.list != snapshot
    }

    pub fn set_default(&mut self, id: &str) -> bool {
        if !self.has(id) || self.default == id {
            return false;
        }
        self.default = id.to_string();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(items: &[&str]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|s| (s.to_string(), s.to_string()))
            .collect()
    }
    fn ids(sp: &Spaces) -> Vec<&str> {
        sp.list.iter().map(|s| s.id.as_str()).collect()
    }

    #[test]
    fn first_run_is_general_plus_existing_clusters_only() {
        let mut sp = Spaces::default();
        let circles = c(&[
            "paceline-planning",
            "paceline-weekend",
            "mtg-roadmap",
            "wander",
            "ui-a",
            "ui-b",
            "claude-49",
            "claude-50",
        ]);
        assert!(sp.reconcile(&circles, None));
        assert_eq!(ids(&sp), vec!["general", "paceline", "ui"]);
        assert_eq!(
            sp.space_of("mtg-roadmap"),
            "general",
            "one circle is not a cluster"
        );
        assert_eq!(
            sp.space_of("claude-49"),
            "general",
            "generated names never route"
        );
        assert_eq!(sp.space_of("ui-b"), "ui");
        assert!(sp.placed.is_empty());
        assert!(!sp.reconcile(&circles, None), "idempotent");
    }

    #[test]
    fn two_windows_reconciling_in_different_orders_agree() {
        let a = c(&["x-1a", "x-2b", "y", "z-q"]);
        let mut b = a.clone();
        b.reverse();
        let (mut s1, mut s2) = (Spaces::default(), Spaces::default());
        s1.reconcile(&a, None);
        s2.reconcile(&b, None);
        assert_eq!(s1, s2);
    }

    #[test]
    fn a_new_circle_lands_in_the_focused_space_unless_a_prefix_claims_it() {
        let mut sp = Spaces::default();
        sp.reconcile(&c(&["a"]), None);
        let w = sp.create("work");
        sp.reconcile(&c(&["a", "b"]), Some(&w));
        assert_eq!(sp.space_of("b"), w);
        sp.reconcile(&c(&["a", "b", "ui-1x", "ui-2y"]), Some(&w));
        assert_eq!(sp.space_of("ui-1x"), "ui", "≥2 shared prefix mints a space");
    }

    #[test]
    fn second_circle_with_a_prefix_mints_the_space_and_pulls_the_first() {
        let mut sp = Spaces::default();
        sp.reconcile(&c(&["mtg-a", "other"]), None);
        assert_eq!(sp.space_of("mtg-a"), "general");
        sp.reconcile(&c(&["mtg-a", "mtg-b", "other"]), None);
        assert_eq!(sp.space_of("mtg-a"), "mtg");
        assert_eq!(sp.space_of("mtg-b"), "mtg");
    }

    #[test]
    fn a_hand_move_is_sticky_against_prefix_routing() {
        let mut sp = Spaces::default();
        let circles = c(&["ui-a", "ui-b"]);
        sp.reconcile(&circles, None);
        let later = sp.create("later");
        assert!(sp.move_circle("ui-a", &later));
        sp.reconcile(&circles, None);
        assert_eq!(sp.space_of("ui-a"), later);
        // ui-b alone is no longer ≥2 unplaced, but its space already exists.
        assert_eq!(sp.space_of("ui-b"), "ui");
    }

    #[test]
    fn a_space_carrying_the_prefix_catches_new_circles_even_after_rename() {
        let mut sp = Spaces::default();
        sp.reconcile(&c(&["mtg-a", "mtg-b"]), None);
        assert!(sp.rename("mtg", "meetings"));
        assert!(!sp.get("mtg").unwrap().auto, "renaming makes it permanent");
        sp.reconcile(&c(&["mtg-a", "mtg-b", "mtg-c"]), None);
        assert_eq!(sp.space_of("mtg-c"), "mtg");
        assert_eq!(sp.get("mtg").unwrap().name, "meetings");
    }

    #[test]
    fn an_auto_space_vanishes_when_it_empties_a_named_one_does_not() {
        let mut sp = Spaces::default();
        sp.reconcile(&c(&["ui-a", "ui-b", "x"]), None);
        sp.reconcile(&c(&["x"]), None);
        assert_eq!(ids(&sp), vec!["general"]);
        let k = sp.create("keep");
        sp.reconcile(&c(&["x"]), None);
        assert!(sp.get(&k).is_some());
    }

    #[test]
    fn renaming_out_of_an_auto_prefix_returns_to_general() {
        let mut sp = Spaces::default();
        sp.reconcile(&c(&["ui-a", "ui-b", "ui-c"]), None);
        let renamed = vec![
            ("ui-a".to_string(), "fresh".to_string()),
            ("ui-b".to_string(), "ui-b".to_string()),
            ("ui-c".to_string(), "ui-c".to_string()),
        ];
        sp.reconcile(&renamed, None);
        assert_eq!(sp.space_of("ui-a"), "general");
    }

    #[test]
    fn deleting_a_space_drops_its_circles_to_the_default_for_good() {
        let mut sp = Spaces::default();
        sp.reconcile(&c(&["ui-a", "ui-b"]), None);
        let p = sp.create("paceline");
        sp.move_circle("ui-a", &p);
        assert!(sp.delete(&p));
        sp.reconcile(&c(&["ui-a", "ui-b"]), None);
        assert_eq!(sp.space_of("ui-a"), "general");
        assert!(!sp.delete("general"), "the default can't be deleted");
    }

    #[test]
    fn the_default_can_move_and_be_renamed() {
        let mut sp = Spaces::default();
        sp.reconcile(&c(&["a"]), None);
        assert!(sp.rename("general", "misc WIP"));
        let w = sp.create("work");
        assert!(sp.set_default(&w));
        assert!(sp.delete("general"));
        assert_eq!(sp.space_of("a"), w);
    }

    #[test]
    fn reorder_moves_a_space_before_another_or_to_the_end() {
        let mut sp = Spaces::default();
        sp.reconcile(&c(&["a"]), None);
        let x = sp.create("x");
        let y = sp.create("y");
        assert!(sp.reorder(&y, Some("general")));
        assert_eq!(ids(&sp), vec![y.as_str(), "general", x.as_str()]);
        assert!(sp.reorder(&y, None));
        assert_eq!(ids(&sp), vec!["general", x.as_str(), y.as_str()]);
        assert!(!sp.reorder(&y, Some(&y)));
    }

    #[test]
    fn resolve_takes_an_id_or_a_unique_name() {
        let mut sp = Spaces::default();
        sp.reconcile(&c(&["a"]), None);
        let m = sp.create("Meetings");
        assert_eq!(sp.resolve(&m).as_deref(), Some(m.as_str()));
        assert_eq!(sp.resolve("meetings").as_deref(), Some(m.as_str()));
        assert_eq!(sp.resolve("nope"), None);
        let dup = sp.create("meetings");
        assert_ne!(dup, m, "ids stay unique");
        assert_eq!(sp.resolve("MEETINGS"), None, "ambiguous name");
        assert_eq!(
            sp.resolve(&m).as_deref(),
            Some(m.as_str()),
            "an exact id still wins"
        );
    }

    #[test]
    fn dead_circles_are_forgotten() {
        let mut sp = Spaces::default();
        sp.reconcile(&c(&["a", "b"]), None);
        sp.move_circle("b", "general");
        sp.reconcile(&c(&["a"]), None);
        assert!(!sp.of.contains_key("b"));
        assert!(!sp.placed.contains("b"));
    }

    #[test]
    fn generated_names_are_not_prefixes() {
        assert_eq!(space_prefix("claude-49"), None);
        assert_eq!(space_prefix("codex-3"), None);
        assert_eq!(space_prefix("ui-6726-free-rp").as_deref(), Some("ui"));
        assert_eq!(space_prefix("mtg-ai").as_deref(), Some("mtg"));
        assert_eq!(space_prefix("solo"), None);
    }

    #[test]
    fn json_roundtrip_and_empty_blob() {
        let mut sp = Spaces::default();
        sp.reconcile(&c(&["ui-a", "ui-b"]), None);
        let back: Spaces = serde_json::from_str(&serde_json::to_string(&sp).unwrap()).unwrap();
        assert_eq!(back, sp);
        let empty: Spaces = serde_json::from_str("{}").unwrap();
        assert!(empty.is_empty());
    }
}
