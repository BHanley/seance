//! Which rows of a grid are Claude Code's own reply, so the GUI can paint
//! them apart from tool output (Brian's ask, 2026-10-30: "so I can tell what
//! is important to read vs terminal output").
//!
//! Claude Code draws its prose and a command's output in the same plain
//! colour, so only the layout tells them apart: a reply opens with `● ` at
//! column 0 in Claude's text colour, while a tool call's dot is green, red or
//! dim. Everything below it, up to the next row that starts at column 0, is
//! that reply.

use crate::links::plain_style;
use crate::snapshot::GridSnapshot;

/// How far, per channel, a reply dot's colour may sit from the plain text
/// colour. Claude may send the 256-colour cube's nearest match instead of
/// the exact RGB; a tool's dim dot is further off than this.
const DOT_SLACK: u32 = 48;

/// One flag per row: true for the rows of a Claude reply. Rows above the
/// first column-0 row on screen are left out, since their owner scrolled off.
pub fn claude_reply_rows(snap: &GridSnapshot) -> Vec<bool> {
    let cols = snap.cols as usize;
    if cols < 2 || snap.cells.is_empty() {
        return Vec::new();
    }
    let rows = snap.cells.len() / cols;
    let (plain_fg, _) = plain_style(snap);
    let mut out = vec![false; rows];
    let mut in_reply = false;
    for (r, flag) in out.iter_mut().enumerate() {
        let row = &snap.cells[r * cols..(r + 1) * cols];
        if row[0].c != ' ' {
            in_reply = matches!(row[0].c, '●' | '⏺')
                && row[1].c == ' '
                && !row[0].dim
                && near(row[0].fg, plain_fg);
        }
        *flag = in_reply;
    }
    out
}

fn near(a: u32, b: u32) -> bool {
    if a == b {
        return true;
    }
    if a == 0xFFFF_FFFF || b == 0xFFFF_FFFF {
        return false;
    }
    (0..3).all(|i| {
        let (x, y) = ((a >> (i * 8)) & 0xff, (b >> (i * 8)) & 0xff);
        x.abs_diff(y) <= DOT_SLACK
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::CellSnap;

    const FG: u32 = 0x00E3_E7EE;
    const BG: u32 = 0x0030_353F;

    /// Rows as Claude Code lays them out, every cell in the plain colours.
    fn grid(cols: usize, lines: &[&str]) -> GridSnapshot {
        let mut snap = GridSnapshot::empty("t-1");
        snap.cols = cols as u16;
        snap.rows = lines.len() as u16;
        for line in lines {
            let mut chars: Vec<char> = line.chars().collect();
            chars.resize(cols, ' ');
            for c in chars {
                let mut cell = CellSnap::blank();
                cell.c = c;
                cell.fg = FG;
                cell.bg = BG;
                snap.cells.push(cell);
            }
        }
        snap
    }

    fn dot(snap: &mut GridSnapshot, row: usize, fg: u32) {
        let cols = snap.cols as usize;
        snap.cells[row * cols].fg = fg;
    }

    #[test]
    fn a_reply_runs_to_the_next_row_at_column_zero() {
        let mut snap = grid(
            30,
            &[
                "  tail of something above",
                "❯ fix the build",
                "● Bash(cargo build)",
                "  ⎿  error: missing ;",
                "● Fixed it: the semicolon was",
                "  missing in main.rs.",
                "",
                "  - Build passes now.",
                "✻ Brewed for 11s",
            ],
        );
        dot(&mut snap, 2, 0x0078_D787); // a tool's green dot
        dot(&mut snap, 4, 0x00FF_FFFF); // Claude's text colour, near the plain fg
        let got = claude_reply_rows(&snap);
        assert_eq!(
            got,
            vec![false, false, false, false, true, true, true, true, false]
        );
    }

    #[test]
    fn a_dim_or_grey_dot_is_a_running_tool() {
        let mut snap = grid(30, &["● Read(src/main.rs)", "  ⎿  Read 40 lines"]);
        dot(&mut snap, 0, 0x0099_9999);
        assert_eq!(claude_reply_rows(&snap), vec![false, false]);
        dot(&mut snap, 0, FG);
        snap.cells[0].dim = true;
        assert_eq!(claude_reply_rows(&snap), vec![false, false]);
    }
}
