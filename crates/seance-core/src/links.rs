//! URL detection over a rendered grid — what a ctrl+click (native) or a tap
//! (web) resolves to.
//!
//! Shared so the native GUI and the web client cannot drift, same rule as
//! `util::title_looks_busy`.

use crate::snapshot::GridSnapshot;

/// Rows joined either side of the hit row while each one continues. A
/// snapshot carries no WRAPPED flag, so continuation is inferred (see
/// [`continues`]); the cap bounds a screen of box-drawing from being glued
/// into one line.
const STITCH_MAX: u16 = 6;

fn is_url_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "-._~:/?#[]@!$&'()*+,;=%".contains(c)
}

/// The link under one cell: an OSC-8 span covering it, else a bare http(s) URL
/// whose extent covers that column.
///
/// Cell-targeted on purpose. This used to open the first link anywhere on
/// screen and ignore the position, so ctrl+clicking a URL in a pane that also
/// had a PR reference higher up opened the PR.
pub fn url_at_cell(snap: &GridSnapshot, row: u16, col: u16) -> Option<String> {
    if let Some(h) = snap
        .hyperlinks
        .iter()
        .find(|h| h.row == row && col >= h.col_start && col < h.col_end)
    {
        return Some(h.uri.clone());
    }
    let (line, hit) = logical_line(snap, row, col)?;
    http_url_at(&line, hit)
}

/// How row `r` flows into row `r + 1`, if it does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Wrap {
    /// The terminal wrapped: the row is full to its last column. Joined raw.
    Soft,
    /// The program wrapped (Claude/ink break a long line at their text-box
    /// width, short of the edge, and indent the rest): the row's text ends in
    /// the middle of a URL and the next row, after its indent, carries on
    /// with a URL-shaped token. Joined without the gap.
    Hard,
}

fn row_chars(snap: &GridSnapshot, r: u16) -> &[crate::snapshot::CellSnap] {
    let cols = snap.cols as usize;
    &snap.cells[r as usize * cols..(r as usize + 1) * cols]
}

/// Does row `r` carry on into `r + 1`?
///
/// The hard case is a guess from the text alone, tuned to never glue a URL
/// onto prose: row `r` must END in URL characters (no closing `.`, `)` …)
/// and row `r + 1` must OPEN with a token of URL characters that is not a
/// plain word (it has a digit, `/`, `.` …) — `…/ri` + `dewithgps/pull/1`
/// joins, `…/x` + `tra text` does not. Only the URL covering the hit is ever
/// returned, so a join elsewhere on the row is harmless.
fn continues(snap: &GridSnapshot, r: u16) -> Option<Wrap> {
    let rows = (snap.cells.len() / snap.cols.max(1) as usize) as u16;
    if r + 1 >= rows {
        return None;
    }
    let this = row_chars(snap, r);
    if this.last().is_some_and(|c| c.c != ' ') {
        return Some(Wrap::Soft);
    }
    let text: String = this.iter().map(|c| c.c).collect();
    let last = text.trim_end().rsplit(' ').next().unwrap_or("");
    let ends_mid_url = !last.is_empty()
        && last.chars().all(is_url_char)
        && !last.ends_with(['.', ',', ';', ':', ')', ']', '!', '?', '\''])
        && (last.contains("://") || last.contains('/') || last.contains('.'));
    if !ends_mid_url {
        return None;
    }
    let next: String = row_chars(snap, r + 1).iter().map(|c| c.c).collect();
    let first = next.trim_start().split(' ').next().unwrap_or("");
    // A list bullet is the start of a new item, never the rest of a URL:
    // "…?item=289" over "- #259 club rides" opened `…item=289-` (2026-10-03).
    // Numbered: 1–3 digits + `.`/`)` with the item's text after it — so a
    // wrapped `…p179026479900` + `6399)` is still a continuation.
    let numbered = (2..=4).contains(&first.len())
        && first[..first.len() - 1].chars().all(|c| c.is_ascii_digit())
        && first.ends_with(['.', ')'])
        && !next.trim_start()[first.len()..].trim().is_empty();
    let bullet = matches!(first, "-" | "*" | "+" | "•") || numbered;
    let continuation = !first.is_empty()
        && !bullet
        && next.starts_with(' ') == text.starts_with(' ')
        && first.chars().all(is_url_char)
        && first.chars().any(|c| c.is_ascii_alphanumeric())
        && !first.chars().all(|c| c.is_ascii_alphabetic());
    continuation.then_some(Wrap::Hard)
}

/// The wrapped run of rows containing `row`, as one string, plus where `col`
/// landed in it. A phone terminal is ~45 columns wide, so any URL worth
/// tapping is wrapped — resolving one row alone hands back a fragment that
/// opens nothing.
fn logical_line(snap: &GridSnapshot, row: u16, col: u16) -> Option<(String, usize)> {
    let cols = snap.cols as usize;
    if cols == 0 || snap.cells.is_empty() {
        return None;
    }
    let rows = (snap.cells.len() / cols) as u16;
    if row >= rows {
        return None;
    }
    let mut start = row;
    while start > 0 && row - start < STITCH_MAX && continues(snap, start - 1).is_some() {
        start -= 1;
    }
    let mut end = row;
    while end - row < STITCH_MAX && continues(snap, end).is_some() {
        end += 1;
    }

    let mut line: Vec<char> = Vec::with_capacity(cols * (end - start + 1) as usize);
    let mut hit = 0;
    let mut joined_hard = false;
    for r in start..=end {
        let mut cells: Vec<char> = row_chars(snap, r).iter().map(|c| c.c).collect();
        let mut lead = 0;
        if joined_hard {
            lead = cells.iter().take_while(|c| **c == ' ').count();
            cells.drain(..lead);
        }
        let hard = continues(snap, r) == Some(Wrap::Hard) && r < end;
        if hard {
            while cells.last() == Some(&' ') {
                cells.pop();
            }
        }
        if r == row {
            hit = line.len() + (col as usize).saturating_sub(lead);
        }
        line.extend(cells);
        joined_hard = hard;
    }
    Some((line.into_iter().collect(), hit))
}

/// The http(s) URL in `line` whose character range covers `col`.
///
/// Indexes by CHAR, not byte: a row of terminal cells is one char per column,
/// so a non-ASCII glyph anywhere left of the URL would slide every byte offset
/// off by one.
pub fn http_url_at(line: &str, col: usize) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();
    let starts_at = |i: usize| {
        chars[i..].starts_with(&['h', 't', 't', 'p', 's', ':', '/', '/'])
            || chars[i..].starts_with(&['h', 't', 't', 'p', ':', '/', '/'])
    };
    let mut i = 0;
    while i < chars.len() {
        if !starts_at(i) {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && is_url_char(chars[i]) {
            i += 1;
        }
        let url: String = chars[start..i].iter().collect();
        let url = url.trim_end_matches(['.', ',', ')', ']', ';', ':']);
        if url.len() > 10 && col >= start && col < start + url.chars().count() {
            return Some(url.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{CellSnap, HyperlinkSpan};

    /// A grid holding one line of text per row, padded to `cols`.
    fn grid_of(cols: usize, lines: &[&str]) -> GridSnapshot {
        let mut snap = GridSnapshot::empty("t-1");
        snap.cols = cols as u16;
        snap.rows = lines.len() as u16;
        snap.cells = Vec::with_capacity(cols * lines.len());
        for line in lines {
            let mut chars: Vec<char> = line.chars().collect();
            chars.resize(cols, ' ');
            for c in chars {
                let mut cell = CellSnap::blank();
                cell.c = c;
                snap.cells.push(cell);
            }
        }
        snap
    }

    /// The regression: ctrl+click used to ignore where you clicked and open
    /// the first link on screen, so clicking a plat URL under a PR reference
    /// opened the PR.
    #[test]
    fn a_click_resolves_the_url_under_the_cursor_not_the_first_on_screen() {
        let snap = grid_of(
            80,
            &[
                "see https://github.com/o/r/pull/6807 for the CI run",
                "plat https://cadence.ham.xyz/plats/86ff368c-92db for the numbers",
            ],
        );
        assert_eq!(
            url_at_cell(&snap, 1, 10).as_deref(),
            Some("https://cadence.ham.xyz/plats/86ff368c-92db")
        );
        assert_eq!(
            url_at_cell(&snap, 0, 10).as_deref(),
            Some("https://github.com/o/r/pull/6807")
        );
    }

    /// Clicking off any link opens nothing — better than opening something
    /// arbitrary from elsewhere on screen.
    #[test]
    fn a_click_on_plain_text_opens_nothing() {
        let snap = grid_of(80, &["plat https://cadence.ham.xyz/x for the numbers"]);
        assert_eq!(url_at_cell(&snap, 0, 0), None);
        assert_eq!(url_at_cell(&snap, 0, 40), None);
    }

    /// An OSC-8 span wins over bare text, but only on the cells it covers.
    #[test]
    fn an_osc8_span_only_claims_its_own_cells() {
        let mut snap = grid_of(80, &["#6807 and https://cadence.ham.xyz/plats/abc"]);
        snap.hyperlinks.push(HyperlinkSpan {
            row: 0,
            col_start: 0,
            col_end: 5,
            uri: "https://github.com/o/r/pull/6807".into(),
        });
        assert_eq!(
            url_at_cell(&snap, 0, 2).as_deref(),
            Some("https://github.com/o/r/pull/6807")
        );
        assert_eq!(
            url_at_cell(&snap, 0, 20).as_deref(),
            Some("https://cadence.ham.xyz/plats/abc")
        );
    }

    /// Column is a CHAR offset, not a byte one — a wide glyph left of the URL
    /// would otherwise slide the hit box.
    #[test]
    fn a_non_ascii_glyph_does_not_shift_the_hit_box() {
        let snap = grid_of(80, &["✦ ✦ https://cadence.ham.xyz/plats/abc"]);
        assert_eq!(
            url_at_cell(&snap, 0, 4).as_deref(),
            Some("https://cadence.ham.xyz/plats/abc")
        );
        assert_eq!(url_at_cell(&snap, 0, 3), None);
    }

    /// The phone case: 44 columns wide, so the URL is in three pieces. A tap
    /// on ANY of them has to produce the whole thing.
    #[test]
    fn a_wrapped_url_is_stitched_back_together_from_any_row() {
        // 40 cols. Row 0 is full to the last column — the wrap signal — and
        // row 1 is not, which ends the logical line.
        let snap = grid_of(
            40,
            &[
                "opened https://github.com/ridewithgps/rw",
                "gps/pull/12345 and the CI is green",
            ],
        );
        let want = Some("https://github.com/ridewithgps/rwgps/pull/12345");
        assert_eq!(url_at_cell(&snap, 0, 10).as_deref(), want);
        assert_eq!(url_at_cell(&snap, 1, 2).as_deref(), want);
    }

    /// Claude/ink wrap a long URL themselves — short of the right edge, with
    /// the rest indented — which the full-row rule never saw. Real frames
    /// from 2026-09-28; a tap on any piece must open the whole URL.
    #[test]
    fn a_program_wrapped_url_is_stitched_across_its_indent() {
        let snap = grid_of(
            48,
            &[
                "     prod. https://github.com/ridewithgps/ri",
                "     dewithgps/pull/23996",
            ],
        );
        let want = Some("https://github.com/ridewithgps/ridewithgps/pull/23996");
        assert_eq!(url_at_cell(&snap, 0, 20).as_deref(), want);
        assert_eq!(url_at_cell(&snap, 1, 8).as_deref(), want);

        // Word-wrapped at a '/', far from the edge, prose after it.
        let snap = grid_of(
            50,
            &[
                "  v2 is live at https://vita-reports.ham.xyz",
                "  /s/675a4e6f8887. There are no more",
            ],
        );
        let want = Some("https://vita-reports.ham.xyz/s/675a4e6f8887");
        assert_eq!(url_at_cell(&snap, 0, 20).as_deref(), want);
        assert_eq!(url_at_cell(&snap, 1, 4).as_deref(), want);

        // Markdown hanging indent; the closing paren is not part of the URL.
        let snap = grid_of(
            80,
            &[
                "      41  - [thread](https://rwgps.slack.com/archives/C05RT3C611P/p179026479900",
                "          6399)",
            ],
        );
        assert_eq!(
            url_at_cell(&snap, 0, 30).as_deref(),
            Some("https://rwgps.slack.com/archives/C05RT3C611P/p1790264799006399")
        );
    }

    /// A markdown list of URLs, one per line (2026-10-03): the next item's
    /// bullet is not the rest of this URL.
    #[test]
    fn a_list_bullet_on_the_next_line_is_not_part_of_the_url() {
        let snap = grid_of(
            80,
            &[
                "- #289 personal chat link: http://localhost:5390/#/board?item=289",
                "- #259 club rides: http://localhost:5390/#/board?item=259",
                "1. http://a.io/x?id=7",
                "2. http://b.io/y",
                "* http://c.io/z?q=1",
                "+ more",
            ],
        );
        assert_eq!(
            url_at_cell(&snap, 0, 40).as_deref(),
            Some("http://localhost:5390/#/board?item=289")
        );
        assert_eq!(
            url_at_cell(&snap, 1, 30).as_deref(),
            Some("http://localhost:5390/#/board?item=259")
        );
        assert_eq!(
            url_at_cell(&snap, 2, 6).as_deref(),
            Some("http://a.io/x?id=7")
        );
        assert_eq!(
            url_at_cell(&snap, 4, 4).as_deref(),
            Some("http://c.io/z?q=1")
        );
    }

    /// Never glue a finished URL onto the next line's prose.
    #[test]
    fn a_url_that_ends_its_line_is_not_glued_to_prose() {
        let snap = grid_of(
            50,
            &[
                "  see https://a.io/x",
                "  tra text here",
                "  https://b.io/y.",
                "  2 more",
            ],
        );
        assert_eq!(url_at_cell(&snap, 0, 8).as_deref(), Some("https://a.io/x"));
        // Sentence-final '.' closes it even though "2" looks like a continuation.
        assert_eq!(url_at_cell(&snap, 2, 5).as_deref(), Some("https://b.io/y"));
    }

    /// Stitching must not walk off into unrelated rows: a row that ends in a
    /// blank is a line break, so the URL stops there.
    #[test]
    fn a_line_ending_in_blank_does_not_swallow_the_next_row() {
        let snap = grid_of(20, &["see https://a.io/x", "tra text here"]);
        assert_eq!(url_at_cell(&snap, 0, 6).as_deref(), Some("https://a.io/x"));
    }
}
