//! Recovering the three SGR attributes vt100 drops, and the colon-form colours it does not read.
//!
//! PORTED from termlens 0.11.2, `src/emu/shadow.rs` (github.com/vyncint/termlens), under its MIT
//! licence, reproduced below as it requires. Modifications for this kernel: the scrollback hooks
//! are dropped (the kernel keeps no styled history), the visibility is `pub(crate)`, the
//! `unhandled`-tracker cross-check test is dropped (there is no such tracker here), and these notes
//! are shortened.
//!
//! ```text
//! MIT License
//!
//! Copyright (c) 2026 Vyncint Ng
//!
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"), to deal
//! in the Software without restriction, including without limitation the rights
//! to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//! copies of the Software, and to permit persons to whom the Software is
//! furnished to do so, subject to the following conditions:
//!
//! The above copyright notice and this permission notice shall be included in all
//! copies or substantial portions of the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//! SOFTWARE.
//! ```
//!
//! vt100 0.16's SGR dispatch handles only `0 1 2 3 4 7 22 23 24 27` and the colours. So `5`/`6`
//! (blink), `8` (conceal) and `9` (strikethrough), with their resets `25`/`28`/`29`, never reach
//! a cell. A test asserting that a password field is masked would then pass against a program
//! that printed the secret in clear.
//!
//! **The fix is a second vt100 parser.** It is fed the same byte stream with only the complete
//! plain SGR sequences rewritten, so three attributes vt100 *does* keep carry the three it drops:
//!
//! | dropped        | carrier       | off       |
//! |----------------|---------------|-----------|
//! | `5`/`6` blink  | `1` bold      | `25`→`22` |
//! | `8` conceal    | `3` italic    | `28`→`23` |
//! | `9` strike     | `4` underline | `29`→`24` |
//!
//! **Why the grids line up.** In vt100 an attribute never influences geometry, and an SGR never
//! moves the cursor. A stream that differs only in whole plain-SGR sequences therefore produces an
//! identically shaped grid, so shadow cell `(r, c)` is primary cell `(r, c)`. The kernel
//! debug-asserts that correspondence on every snapshot.

/// Where the rewriter is in the byte stream — only enough to recognize a complete plain SGR.
#[derive(Debug, PartialEq, Eq)]
enum State {
    Ground,
    Esc,
    Csi,
}

/// Rewrites colon-form extended colours (`38:2::r:g:b`, `38:5:n`) into the semicolon form vt100
/// reads. Holds only a complete plain SGR; every other sequence is emitted unchanged.
pub(crate) struct ColorNormalizer {
    state: State,
    pending: Vec<u8>,
}

impl ColorNormalizer {
    pub(crate) fn new() -> Self {
        Self { state: State::Ground, pending: Vec::new() }
    }

    pub(crate) fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        rewrite_stream(&mut self.state, &mut self.pending, bytes, normalize_colon_colours)
    }
}

/// A parallel vt100 parser whose bold/italic/underline are the primary's blink/conceal/strike.
pub(crate) struct AttrShadow {
    parser: vt100::Parser,
    state: State,
    /// Bytes held back while they might be a plain SGR. Always emitted in the end — rewritten if
    /// they are one, verbatim if not — so the shadow never loses a byte that shapes the grid.
    pending: Vec<u8>,
}

impl AttrShadow {
    pub(crate) fn new(rows: u16, cols: u16) -> Self {
        Self { parser: vt100::Parser::new(rows, cols, 0), state: State::Ground, pending: Vec::new() }
    }

    pub(crate) fn set_size(&mut self, rows: u16, cols: u16) {
        self.parser.screen_mut().set_size(rows, cols);
    }

    /// The shadow cell at `(row, col)`: bold = blink, italic = conceal, underline = strike.
    pub(crate) fn cell(&self, row: u16, col: u16) -> Option<&vt100::Cell> {
        self.parser.screen().cell(row, col)
    }

    /// The shadow grid's text, for the correspondence check on every snapshot.
    pub(crate) fn contents(&self) -> String {
        self.parser.screen().contents()
    }

    pub(crate) fn feed(&mut self, bytes: &[u8]) {
        let shadowed = rewrite_stream(&mut self.state, &mut self.pending, bytes, rewrite_sgr);
        self.parser.process(&shadowed);
    }
}

fn rewrite_stream(
    state: &mut State,
    pending: &mut Vec<u8>,
    bytes: &[u8],
    rewrite: fn(&[u8]) -> Option<Vec<u8>>,
) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    for &b in bytes {
        // An ESC anywhere abandons whatever was being collected: the held bytes were not an SGR.
        if b == 0x1b {
            out.append(pending);
            pending.push(b);
            *state = State::Esc;
            continue;
        }
        match state {
            State::Ground => out.push(b),
            State::Esc => {
                pending.push(b);
                if b == b'[' {
                    *state = State::Csi;
                } else {
                    out.append(pending);
                    *state = State::Ground;
                }
            }
            State::Csi => {
                pending.push(b);
                match b {
                    // Parameter and intermediate bytes: keep collecting.
                    0x20..=0x3f => {}
                    // Final byte: an SGR is rewritten, anything else passes through.
                    0x40..=0x7e => {
                        let seq = std::mem::take(pending);
                        *state = State::Ground;
                        match (b == b'm').then(|| rewrite(&seq)).flatten() {
                            Some(rewritten) => out.extend_from_slice(&rewritten),
                            None => out.extend_from_slice(&seq),
                        }
                    }
                    // A control byte inside a CSI (CAN, SUB, …): vte decides; preserve it.
                    _ => {
                        out.append(pending);
                        *state = State::Ground;
                    }
                }
            }
        }
    }
    out
}

fn normalize_colon_colours(seq: &[u8]) -> Option<Vec<u8>> {
    let params = seq.get(2..seq.len().checked_sub(1)?)?;
    if params.iter().any(|b| !matches!(b, b'0'..=b'9' | b';' | b':')) {
        return None;
    }
    if !params.contains(&b':') {
        return None;
    }
    let groups: Vec<&[u8]> = params.split(|&b| b == b';').collect();
    let mut normalized = Vec::with_capacity(groups.len());
    let mut changed = false;
    for group in groups {
        let parts: Vec<&[u8]> = group.split(|&b| b == b':').collect();
        let replacement = match parts.as_slice() {
            [first @ (b"38" | b"48"), b"2", b"", red, green, blue]
            | [first @ (b"38" | b"48"), b"2", red, green, blue] => {
                Some(vec![*first, b"2", *red, *green, *blue])
            }
            [first @ (b"38" | b"48"), b"5", index] => Some(vec![*first, b"5", *index]),
            _ => None,
        };
        if let Some(replacement) = replacement {
            normalized.extend(replacement.into_iter().map(<[u8]>::to_vec));
            changed = true;
        } else {
            normalized.push(group.to_vec());
        }
    }
    if !changed {
        return None;
    }
    let mut out = Vec::with_capacity(seq.len());
    out.extend_from_slice(b"\x1b[");
    for (index, group) in normalized.iter().enumerate() {
        if index > 0 {
            out.push(b';');
        }
        out.extend_from_slice(group);
    }
    out.push(b'm');
    Some(out)
}

/// Rewrite one complete `ESC [ … m` into the carrier form. `None` means "not a plain SGR — emit it
/// verbatim": a private prefix or an intermediate byte makes it something else, and guessing there
/// is how a rewriter loses bytes that shape the grid. A sequence left with no carrier becomes the
/// surrogate `ESC[39m`, so its ESC survives (it may be terminating an OSC or aborting a CSI).
fn rewrite_sgr(seq: &[u8]) -> Option<Vec<u8>> {
    let params = seq.get(2..seq.len().checked_sub(1)?)?;
    if params.iter().any(|b| !matches!(b, b'0'..=b'9' | b';' | b':')) {
        return None;
    }

    /// The value vte reports for a parameter group: its first sub-parameter, empty meaning zero.
    fn value(group: &[u8]) -> u32 {
        let first = group.split(|&b| b == b':').next().unwrap_or(b"");
        first.iter().fold(0u32, |acc, &d| acc.saturating_mul(10).saturating_add(u32::from(d - b'0')))
    }

    let groups: Vec<&[u8]> = params.split(|&b| b == b';').collect();
    let mut out: Vec<u32> = Vec::new();
    let mut i = 0;
    while i < groups.len() {
        let group = groups[i];
        let v = value(group);
        match v {
            // Extended colour: step over its sub-parameters, or the `5` in `38;5;196` reads as
            // blink and paints a whole line with an attribute the program never set.
            38 | 48 | 58 => {
                if group.contains(&b':') {
                    i += 1;
                } else {
                    i += match groups.get(i + 1).map(|g| value(g)) {
                        Some(2) => 5,
                        Some(5) => 3,
                        _ => 1,
                    };
                }
            }
            _ => {
                if let Some(c) = carrier(v) {
                    out.push(c);
                }
                i += 1;
            }
        }
    }

    let mut bytes = Vec::new();
    if out.is_empty() {
        // `39` resets only the foreground colour, which no shadow cell is ever read for; `0` would
        // clear a carrier mid-run.
        bytes.extend_from_slice(b"\x1b[39m");
        return Some(bytes);
    }
    bytes.extend_from_slice(b"\x1b[");
    for (n, param) in out.iter().enumerate() {
        if n > 0 {
            bytes.push(b';');
        }
        bytes.extend_from_slice(param.to_string().as_bytes());
    }
    bytes.push(b'm');
    Some(bytes)
}

/// The shadow parameter carrying `param`, if any.
fn carrier(param: u32) -> Option<u32> {
    match param {
        0 => Some(0),     // reset all: means the same in both streams
        5 | 6 => Some(1), // blink (slow, rapid) -> bold
        8 => Some(3),     // conceal -> italic
        9 => Some(4),     // strikethrough -> underline
        25 => Some(22),   // blink off -> normal intensity
        28 => Some(23),   // conceal off -> italic off
        29 => Some(24),   // strikethrough off -> underline off
        _ => None,        // everything else belongs to the primary alone
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shadowed(bytes: &[u8]) -> String {
        let mut shadow = AttrShadow::new(4, 20);
        let mut out = rewrite_stream(&mut shadow.state, &mut shadow.pending, bytes, rewrite_sgr);
        out.append(&mut shadow.pending);
        String::from_utf8_lossy(&out).replace('\x1b', "E")
    }

    fn normalized(bytes: &[u8]) -> String {
        let mut state = State::Ground;
        let mut pending = Vec::new();
        let mut out = rewrite_stream(&mut state, &mut pending, bytes, normalize_colon_colours);
        out.append(&mut pending);
        String::from_utf8_lossy(&out).replace('\x1b', "E")
    }

    #[test]
    fn the_three_dropped_attributes_get_carriers() {
        assert_eq!(shadowed(b"\x1b[5mX"), "E[1mX");
        assert_eq!(shadowed(b"\x1b[6mX"), "E[1mX");
        assert_eq!(shadowed(b"\x1b[8mX"), "E[3mX");
        assert_eq!(shadowed(b"\x1b[9mX"), "E[4mX");
        assert_eq!(shadowed(b"\x1b[25m"), "E[22m");
        assert_eq!(shadowed(b"\x1b[28m"), "E[23m");
        assert_eq!(shadowed(b"\x1b[29m"), "E[24m");
    }

    #[test]
    fn the_primarys_own_attributes_are_dropped_from_the_shadow() {
        assert_eq!(shadowed(b"\x1b[1mX"), "E[39mX");
        assert_eq!(shadowed(b"\x1b[7;31;44mX"), "E[39mX");
        assert_eq!(shadowed(b"\x1b[22m\x1b[23m\x1b[24m\x1b[27m"), "E[39mE[39mE[39mE[39m");
        assert_eq!(shadowed(b"\x1b[0mX"), "E[0mX");
        assert_eq!(shadowed(b"\x1b[mX"), "E[0mX");
    }

    #[test]
    fn mixed_parameters_keep_only_the_carriers_in_order() {
        assert_eq!(shadowed(b"\x1b[1;5;31mX"), "E[1mX");
        assert_eq!(shadowed(b"\x1b[0;9;1;8mX"), "E[0;4;3mX");
    }

    #[test]
    fn a_dropped_sgr_keeps_a_structural_esc_in_step() {
        for payload in [&b"\x1b]0;title\x1b[31mA\x1b[5mB\x1b[0mC"[..], &b"\x1b[1;2\x1b[31mA"[..]] {
            let mut primary = vt100::Parser::new(4, 20, 0);
            let mut shadow = AttrShadow::new(4, 20);
            primary.process(payload);
            shadow.feed(payload);
            assert_eq!(primary.screen().contents(), shadow.contents(), "payload {payload:?}");
        }
    }

    #[test]
    fn an_extended_colour_never_looks_like_a_carrier() {
        assert_eq!(shadowed(b"\x1b[38;5;196mX"), "E[39mX");
        assert_eq!(shadowed(b"\x1b[38;2;255;0;8mX"), "E[39mX");
        assert_eq!(shadowed(b"\x1b[38;2;0;9;0;5mX"), "E[1mX");
        assert_eq!(shadowed(b"\x1b[38:5:196mX"), "E[39mX");
        assert_eq!(shadowed(b"\x1b[4:3mX"), "E[39mX");
        assert_eq!(shadowed(b"\x1b[38;5;196;9mX"), "E[4mX");
    }

    #[test]
    fn anything_that_is_not_a_plain_sgr_passes_through_verbatim() {
        for seq in [
            &b"\x1b[?2026h"[..],
            &b"\x1b[2J"[..],
            &b"\x1b[3;7Hhi"[..],
            &b"\x1b[>4;2m"[..],
            &b"\x1b[4$p"[..],
            &b"\x1b]0;title\x07"[..],
            &b"\x1b7\x1b8"[..],
            &b"plain text\r\n\t"[..],
        ] {
            assert_eq!(shadowed(seq), String::from_utf8_lossy(seq).replace('\x1b', "E"));
        }
    }

    #[test]
    fn colon_colours_normalize_and_everything_else_passes_through() {
        assert_eq!(normalized(b"\x1b[38:2::10:20:30m"), "E[38;2;10;20;30m");
        assert_eq!(normalized(b"\x1b[38:5:196m"), "E[38;5;196m");
        assert_eq!(normalized(b"\x1b[1;38:2::4:5:6;3m"), "E[1;38;2;4;5;6;3m");
        for seq in [&b"\x1b[4:3m"[..], &b"\x1b[38:2::9m"[..], &b"\x1b[?25l"[..]] {
            assert_eq!(normalized(seq), String::from_utf8_lossy(seq).replace('\x1b', "E"));
        }
    }

    #[test]
    fn a_sequence_split_across_feeds_is_not_lost() {
        let mut shadow = AttrShadow::new(2, 10);
        shadow.feed(b"\x1b[8");
        assert!(!shadow.cell(0, 0).is_some_and(vt100::Cell::italic));
        shadow.feed(b"mX");
        assert!(shadow.cell(0, 0).is_some_and(vt100::Cell::italic), "the conceal carrier survives");
    }

    #[test]
    fn an_aborted_csi_keeps_its_bytes() {
        assert_eq!(shadowed(b"\x1b[31\x18X"), "E[31\u{18}X");
        assert_eq!(shadowed(b"\x1b[31\x1b[9mX"), "E[31E[4mX");
    }
}
