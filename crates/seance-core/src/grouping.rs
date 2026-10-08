//! Sidebar sections and prefix grouping — pure, and shared so the native and
//! web rails cannot drift apart.
//!
//! Two independent axes:
//!
//! * **Section** — pinned or not. The rail draws no band headers; pinned rows
//!   sit above a full-width rule and everything else below it. The enum
//!   survives because prefix groups fold per band (`pinned/mtg` is not
//!   `active/mtg`).
//! * **Group** — visual clustering *inside* a section, from the text before the
//!   first `-` in a circle's label. Name three circles `mtg-growth`,
//!   `mtg-ai`, `mtg-carl` and they cluster under `mtg`.
//!
//! Grouping is deliberately a naming convention rather than a stored
//! attribute. You get it by typing, you undo it by typing, and it costs
//! nothing when you don't want it — which is the point when the grouping you
//! want only matters for an afternoon. A prefix carried by just one circle is
//! not a group; it renders as a plain row.
//!
//! Each section groups **independently**: pinned `mtg` circles cluster above
//! the rule, the rest cluster below it, and neither knows about the other.

use std::collections::BTreeSet;

/// Which band a circle renders in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section {
    /// In a host circle mode marked `top` (AFK, on a slack thread): above
    /// everything, one band per mode in host order, so "what did I leave
    /// running while away" is the first thing you see. The index is the
    /// mode's position among the top modes.
    Top(u8),
    /// Explicitly pinned. Wins over every other state — a pin is a statement
    /// about where you want to *look*, not about what the circle is doing.
    Pinned,
    /// Everything else, asleep or not — a slept circle keeps its place and
    /// says so with its own row styling.
    Active,
}

impl Section {
    /// Stable key for persisting group-fold state.
    pub fn key(self) -> &'static str {
        const TOP: [&str; 8] = [
            "top", "top1", "top2", "top3", "top4", "top5", "top6", "top7",
        ];
        match self {
            Section::Top(i) => TOP[(i as usize).min(TOP.len() - 1)],
            Section::Pinned => "pinned",
            Section::Active => "active",
        }
    }
}

/// One rendered entry in a section: a circle on its own, or a cluster of
/// circles sharing a name prefix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SectionRow {
    Circle(String),
    Group {
        /// The shared text before the first `-`, as first typed.
        prefix: String,
        members: Vec<String>,
    },
}

/// Split circles into the bands, top to bottom: one per `top` host mode (AFK,
/// slack thread — `top[i]` is the circles in the i-th), pinned, everything
/// else.
///
/// `ordered` carries the sidebar sort, and every band preserves it. A pinned
/// circle that is asleep stays pinned — you asked for it to be at the top, and
/// the daemon dozing it off is not a reason to move it. A circle in a top mode
/// goes to that mode's band even when pinned, and returns to its pin after; a
/// circle in two top modes sits in the first.
pub fn partition_sections(
    ordered: &[String],
    pinned: &BTreeSet<String>,
    top: &[BTreeSet<String>],
) -> Vec<(Section, Vec<String>)> {
    let mut ups: Vec<Vec<String>> = vec![Vec::new(); top.len()];
    let mut pin = Vec::new();
    let mut act = Vec::new();
    for ws in ordered {
        if let Some(i) = top.iter().position(|set| set.contains(ws)) {
            ups[i].push(ws.clone());
        } else if pinned.contains(ws) {
            pin.push(ws.clone());
        } else {
            act.push(ws.clone());
        }
    }
    let mut out: Vec<(Section, Vec<String>)> = ups
        .into_iter()
        .enumerate()
        .map(|(i, v)| (Section::Top(i as u8), v))
        .collect();
    out.push((Section::Pinned, pin));
    out.push((Section::Active, act));
    out
}

/// The `top` argument of [`partition_sections`]: for each top mode id, in host
/// order, the circles currently in it. `modes` is circle → mode ids on.
pub fn top_mode_bands<'a, I>(top_ids: &[&str], modes: I) -> Vec<BTreeSet<String>>
where
    I: IntoIterator<Item = (&'a String, &'a Vec<String>)>,
{
    let mut bands = vec![BTreeSet::new(); top_ids.len()];
    for (ws, on) in modes {
        for (i, id) in top_ids.iter().enumerate() {
            if on.iter().any(|m| m == id) {
                bands[i].insert(ws.clone());
            }
        }
    }
    bands
}

/// The grouping key of a label: the text before its first `-`, lowercased.
/// `None` when there is no hyphen — an unhyphenated name opts out, which is
/// how you keep a circle loose without thinking about it.
pub fn prefix_of(label: &str) -> Option<String> {
    let (head, _) = label.split_once('-')?;
    let head = head.trim();
    if head.is_empty() {
        return None;
    }
    Some(head.to_ascii_lowercase())
}

/// Cluster one section's circles by name prefix, preserving the incoming sort.
///
/// A group lands at the position of its **first** member, so a cluster floats
/// exactly as high as its most-deserving circle would have on its own — the
/// rail keeps meaning what it meant. Members hold their relative order inside.
/// A prefix with only one circle behind it is not a group.
pub fn group_by_prefix<F>(circles: &[String], label_of: F) -> Vec<SectionRow>
where
    F: Fn(&str) -> String,
{
    let labels: Vec<(String, String)> = circles
        .iter()
        .map(|ws| (ws.clone(), label_of(ws)))
        .collect();

    // Count first: a prefix only earns a header once a second circle shares it.
    let mut counts: Vec<(String, usize)> = Vec::new();
    for (_, label) in &labels {
        if let Some(p) = prefix_of(label) {
            match counts.iter_mut().find(|(k, _)| *k == p) {
                Some((_, n)) => *n += 1,
                None => counts.push((p, 1)),
            }
        }
    }
    let grouped = |p: &str| counts.iter().any(|(k, n)| k == p && *n > 1);

    let mut out: Vec<SectionRow> = Vec::new();
    for (ws, label) in &labels {
        match prefix_of(label).filter(|p| grouped(p)) {
            None => out.push(SectionRow::Circle(ws.clone())),
            Some(key) => {
                // Join the existing cluster, or open one here — which is what
                // pins the group to its first member's position.
                let existing = out.iter_mut().find_map(|row| match row {
                    SectionRow::Group { prefix, members } if prefix.to_ascii_lowercase() == key => {
                        Some(members)
                    }
                    _ => None,
                });
                match existing {
                    Some(members) => members.push(ws.clone()),
                    None => out.push(SectionRow::Group {
                        // Display the prefix as the human typed it here.
                        prefix: label
                            .split_once('-')
                            .map(|(h, _)| h.trim())
                            .unwrap_or("")
                            .into(),
                        members: vec![ws.clone()],
                    }),
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }
    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }
    /// Identity labels — most tests don't need a rename.
    fn same(ws: &str) -> String {
        ws.to_string()
    }

    #[test]
    fn pinned_splits_off_and_everything_else_keeps_its_place() {
        let ordered = v(&["a", "b", "c", "d", "e"]);
        let bands = partition_sections(&ordered, &set(&["a"]), &[]);
        let by = |s: Section| bands.iter().find(|(k, _)| *k == s).unwrap().1.clone();
        assert_eq!(by(Section::Pinned), v(&["a"]));
        // Asleep or not, everything else stays in one band in sort order.
        assert_eq!(by(Section::Active), v(&["b", "c", "d", "e"]));
    }

    /// A circle in a top mode (AFK) leads the rail, even over a pin, and
    /// goes back to its pin when the mode ends.
    #[test]
    fn top_mode_circles_lead_even_over_pins() {
        let ordered = v(&["a", "b", "c", "d"]);
        let bands = partition_sections(&ordered, &set(&["b", "c"]), &[set(&["c", "d"])]);
        let by = |s: Section| bands.iter().find(|(k, _)| *k == s).unwrap().1.clone();
        assert_eq!(by(Section::Top(0)), v(&["c", "d"]));
        assert_eq!(by(Section::Pinned), v(&["b"]));
        assert_eq!(by(Section::Active), v(&["a"]));
        let back = partition_sections(&ordered, &set(&["b", "c"]), &[]);
        assert_eq!(back[0].1, v(&["b", "c"]));
    }

    /// Each top mode is its own band, in host order; a circle in two sits
    /// in the first.
    #[test]
    fn each_top_mode_gets_its_own_band() {
        let ordered = v(&["a", "b", "c", "d"]);
        let bands = partition_sections(&ordered, &set(&[]), &[set(&["b", "c"]), set(&["c", "a"])]);
        let order: Vec<Section> = bands.iter().map(|(s, _)| *s).collect();
        assert_eq!(
            order,
            vec![
                Section::Top(0),
                Section::Top(1),
                Section::Pinned,
                Section::Active
            ]
        );
        assert_eq!(bands[0].1, v(&["b", "c"]));
        assert_eq!(bands[1].1, v(&["a"]));
        assert_eq!(bands[3].1, v(&["d"]));
        assert_ne!(Section::Top(0).key(), Section::Top(1).key());
    }

    /// Band ORDER is a contract, not an implementation detail: "jump to the
    /// top of the rail" (ctrl+shift+home) is the first name of the first
    /// non-empty band, so pinned has to come back first.
    #[test]
    fn bands_come_back_in_rail_order() {
        let bands = partition_sections(&v(&["a", "b", "c", "d"]), &set(&["c"]), &[]);
        let order: Vec<Section> = bands.iter().map(|(s, _)| *s).collect();
        assert_eq!(order, vec![Section::Pinned, Section::Active]);
        // Top of the rail with something pinned is that pinned circle, even
        // though `a` sorts first overall.
        let top = bands
            .iter()
            .find(|(_, ws)| !ws.is_empty())
            .and_then(|(_, ws)| ws.first())
            .cloned();
        assert_eq!(top.as_deref(), Some("c"));
    }

    #[test]
    fn a_prefix_needs_two_circles_to_become_a_group() {
        let rows = group_by_prefix(&v(&["mtg-growth", "solo-thing", "mtg-ai"]), same);
        assert_eq!(
            rows,
            vec![
                SectionRow::Group {
                    prefix: "mtg".into(),
                    members: v(&["mtg-growth", "mtg-ai"]),
                },
                SectionRow::Circle("solo-thing".into()),
            ]
        );
    }

    #[test]
    fn a_group_sits_where_its_first_member_would_have() {
        // Incoming order is the sidebar sort; the cluster must not jump the
        // queue, so it lands exactly where `mtg-growth` was.
        let rows = group_by_prefix(&v(&["urgent", "mtg-growth", "other", "mtg-ai"]), same);
        assert_eq!(
            rows,
            vec![
                SectionRow::Circle("urgent".into()),
                SectionRow::Group {
                    prefix: "mtg".into(),
                    members: v(&["mtg-growth", "mtg-ai"]),
                },
                SectionRow::Circle("other".into()),
            ]
        );
    }

    #[test]
    fn an_unhyphenated_name_opts_out() {
        assert_eq!(prefix_of("mtg"), None);
        assert_eq!(prefix_of("-leading"), None);
        assert_eq!(prefix_of("mtg-growth").as_deref(), Some("mtg"));
        // Only the FIRST hyphen splits, so deeper dashes stay in the tail.
        assert_eq!(prefix_of("stack-misc-a-6553").as_deref(), Some("stack"));
        // A circle named exactly `mtg` does not join the `mtg-` cluster.
        let rows = group_by_prefix(&v(&["mtg", "mtg-a", "mtg-b"]), same);
        assert_eq!(rows[0], SectionRow::Circle("mtg".into()));
        assert!(matches!(&rows[1], SectionRow::Group { members, .. } if members.len() == 2));
    }

    #[test]
    fn grouping_reads_the_label_not_the_slug() {
        // The slug is frozen at creation; the label is what you retype to
        // regroup, so grouping has to follow the label.
        let rows = group_by_prefix(&v(&["growth", "atlas"]), |ws| match ws {
            "growth" => "mtg-growth".into(),
            "atlas" => "mtg-atlas".into(),
            other => other.into(),
        });
        assert_eq!(
            rows,
            vec![SectionRow::Group {
                prefix: "mtg".into(),
                members: v(&["growth", "atlas"]),
            }]
        );
    }

    #[test]
    fn prefix_matching_is_case_insensitive_and_displays_as_typed() {
        let rows = group_by_prefix(&v(&["a", "b"]), |ws| match ws {
            "a" => "MTG-growth".into(),
            _ => "mtg-ai".into(),
        });
        match &rows[0] {
            SectionRow::Group { prefix, members } => {
                assert_eq!(prefix, "MTG", "shown as first typed");
                assert_eq!(members.len(), 2, "matched case-insensitively");
            }
            other => panic!("expected a group, got {other:?}"),
        }
    }

    #[test]
    fn empty_section_yields_no_rows() {
        assert!(group_by_prefix(&[], same).is_empty());
    }
}
