//! The ORACLE: the kernel measured against an external truth (docs/design/terminal-kernel.md#the-oracle).
//!
//! Each fixture is a small program run twice — once through termlens (a dev-dependency, never a
//! runtime one) and once through this kernel — and each screen is read through an ADAPTER that
//! reports every aspect the fixture targets, or `None` where the kernel cannot report it at all.
//! Every place the two disagree is a named key, `fixture/aspect`. The committed list of those keys,
//! `tests/oracle.baseline`, is the kernel's burn-down list, and its length is the disagreement
//! count: a lower-is-better RATCHET. A key that appears and is not listed fails (a regression, or a
//! new fixture whose gaps have not been admitted deliberately). A listed key that no longer
//! disagrees ALSO fails, because the list only ever shrinks: remove it, and the gain is locked in.
//!
//! The pattern is the vim oracle's in anemnez/substrate (crates/cos-input-core/corpus/vim_oracle.vim):
//! never expect from memory — expect what the reference does.

#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use prova_terminal::{Session, SpawnSpec};

const COLS: u16 = 80;
const ROWS: u16 = 24;
const QUIET: Duration = Duration::from_millis(250);
const BOUND: Duration = Duration::from_secs(10);
const BASELINE: &str = include_str!("oracle.baseline");

/// A cell attribute a fixture can target.
#[derive(Clone, Copy, Debug)]
enum Attr {
    Fg,
    Bg,
    Bold,
    Dim,
    Italic,
    Underline,
    Blink,
    Reverse,
    Conceal,
    Strikethrough,
}

/// One aspect of a screen a fixture targets.
#[derive(Clone, Copy, Debug)]
enum Probe {
    /// Every row's text, trailing blanks trimmed, trailing empty rows dropped.
    Text,
    Cell(u16, u16, Attr),
    /// `(row, col)`.
    Cursor,
    CursorVisible,
    /// `default` / `block` / `underline` / `bar`.
    CursorShape,
    Title,
    AlternateScreen,
    BracketedPaste,
    ApplicationCursor,
    /// Whether any mouse reporting is enabled.
    MouseReporting,
}

impl Probe {
    fn key(self) -> String {
        match self {
            Probe::Text => "text".into(),
            Probe::Cell(r, c, a) => format!("cell({r},{c}).{}", format!("{a:?}").to_lowercase()),
            Probe::Cursor => "cursor".into(),
            Probe::CursorVisible => "cursor_visible".into(),
            Probe::CursorShape => "cursor_shape".into(),
            Probe::Title => "title".into(),
            Probe::AlternateScreen => "alternate_screen".into(),
            Probe::BracketedPaste => "bracketed_paste".into(),
            Probe::ApplicationCursor => "application_cursor".into(),
            Probe::MouseReporting => "mouse_reporting".into(),
        }
    }
}

struct Fixture {
    name: &'static str,
    /// The shell for `script`: `sh`, or `bash` where the script needs `read -t`/`-d`.
    shell: &'static str,
    /// Draws, then stays alive until the harness has looked — the screen is read after
    /// `QUIET` of silence, never at a child exit that could race the reader.
    script: &'static str,
    probes: &'static [Probe],
}

use Attr::*;
use Probe::*;

/// The corpus. Every script is POSIX `printf` except the responder's, which needs bash's `read`.
const FIXTURES: &[Fixture] = &[
    Fixture { name: "plain_text", shell: "sh", script: r"printf 'hello\nworld'; sleep 5", probes: &[Text, Cursor] },
    Fixture {
        name: "wrap",
        shell: "sh",
        script: r"printf '%0100d' 0 | tr 0 x; sleep 5",
        probes: &[Text, Cursor],
    },
    Fixture {
        name: "clear_and_home",
        shell: "sh",
        script: r"printf 'junk\033[2J\033[Hclean'; sleep 5",
        probes: &[Text, Cursor],
    },
    Fixture {
        name: "sgr_colors",
        shell: "sh",
        script: r"printf '\033[31mred\033[0m \033[42mgreenbg\033[0m \033[38;5;208midx\033[0m \033[38;2;255;0;16mrgb\033[0m'; sleep 5",
        probes: &[Text, Cell(0, 0, Fg), Cell(0, 4, Bg), Cell(0, 12, Fg), Cell(0, 16, Fg), Cell(0, 3, Fg)],
    },
    Fixture {
        name: "sgr_attrs",
        shell: "sh",
        script: r"printf '\033[1mB\033[0m\033[2mD\033[0m\033[3mI\033[0m\033[4mU\033[0m\033[5mK\033[0m\033[7mR\033[0m\033[8mC\033[0m\033[9mS\033[0m'; sleep 5",
        probes: &[
            Cell(0, 0, Bold),
            Cell(0, 1, Dim),
            Cell(0, 2, Italic),
            Cell(0, 3, Underline),
            Cell(0, 4, Blink),
            Cell(0, 5, Reverse),
            Cell(0, 6, Conceal),
            Cell(0, 7, Strikethrough),
            Cell(0, 8, Bold),
        ],
    },
    Fixture {
        name: "cursor_position",
        shell: "sh",
        script: r"printf '\033[5;10Hx'; sleep 5",
        probes: &[Text, Cursor],
    },
    Fixture {
        name: "cursor_hidden",
        shell: "sh",
        script: r"printf 'h\033[?25l'; sleep 5",
        probes: &[CursorVisible],
    },
    Fixture {
        name: "cursor_shape_bar",
        shell: "sh",
        script: r"printf 'b\033[6 q'; sleep 5",
        probes: &[CursorShape],
    },
    Fixture {
        name: "cursor_shape_block",
        shell: "sh",
        script: r"printf 'b\033[2 q'; sleep 5",
        probes: &[CursorShape],
    },
    Fixture {
        name: "title",
        shell: "sh",
        script: r"printf 't\033]0;oracle-title\007'; sleep 5",
        probes: &[Title],
    },
    Fixture {
        name: "alternate_screen",
        shell: "sh",
        script: r"printf 'main'; printf '\033[?1049h\033[Halt'; sleep 5",
        probes: &[Text, AlternateScreen],
    },
    Fixture {
        name: "input_modes",
        shell: "sh",
        script: r"printf 'm\033[?2004h\033[?1h\033[?1000h'; sleep 5",
        probes: &[BracketedPaste, ApplicationCursor, MouseReporting],
    },
    Fixture {
        // A capability probe: primary device attributes. A terminal that never answers leaves the
        // program waiting out its timeout — the hang termlens's responder exists to prevent.
        name: "query_device_attributes",
        shell: "bash",
        // Echo goes off BEFORE the query: with `read -s` alone the reply can arrive while the tty
        // still echoes, and the screen then shows it twice or once depending on a race.
        script: r#"stty -echo; printf '\033[c'; IFS= read -r -t 2 -d c reply; printf 'reply:%s' "${reply#?}"; sleep 5"#,
        probes: &[Text],
    },
    Fixture {
        name: "query_secondary_da",
        shell: "bash",
        script: r#"stty -echo; printf '\033[>c'; IFS= read -r -t 2 -d c reply; printf 'reply:%s' "${reply#?}"; sleep 5"#,
        probes: &[Text],
    },
    Fixture {
        name: "query_operating_status",
        shell: "bash",
        script: r#"stty -echo; printf '\033[5n'; IFS= read -r -t 2 -d n reply; printf 'reply:%s' "${reply#?}"; sleep 5"#,
        probes: &[Text],
    },
    Fixture {
        // The cursor stands at row 3, column 5 when it asks; the answer is 1-based.
        name: "query_cursor_position",
        shell: "bash",
        script: r#"stty -echo; printf '\033[3;5H\033[6n'; IFS= read -r -t 2 -d R reply; printf '\033[Hreply:%s' "${reply#?}"; sleep 5"#,
        probes: &[Text],
    },
    Fixture {
        name: "query_text_area_size",
        shell: "bash",
        script: r#"stty -echo; printf '\033[18t'; IFS= read -r -t 2 -d t reply; printf 'reply:%s' "${reply#?}"; sleep 5"#,
        probes: &[Text],
    },
];

/// The colour vocabulary the kernel speaks (its Cell.fg/bg), for termlens's colours.
fn lens_color(c: termlens::Color) -> String {
    const NAMES: [&str; 16] = [
        "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white", "bright-black",
        "bright-red", "bright-green", "bright-yellow", "bright-blue", "bright-magenta", "bright-cyan",
        "bright-white",
    ];
    match c {
        termlens::Color::Default => "default".into(),
        termlens::Color::Indexed(i) if usize::from(i) < NAMES.len() => NAMES[usize::from(i)].into(),
        termlens::Color::Indexed(i) => format!("idx-{i}"),
        termlens::Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
    }
}

fn rows_text(rows: impl Iterator<Item = String>) -> String {
    let lines: Vec<String> = rows.map(|l| l.trim_end().to_string()).collect();
    lines.join("\n").trim_end_matches('\n').to_string()
}

/// What termlens reports for a probe — always something: it is the reference.
fn lens_probe(s: &termlens::Screen, p: Probe) -> Option<String> {
    Some(match p {
        Text => rows_text((0..s.rows()).map(|r| s.row_text(r))),
        Cell(r, c, a) => {
            let st = *s.cell(r, c)?.style();
            match a {
                Fg => lens_color(st.fg),
                Bg => lens_color(st.bg),
                Bold => st.bold.to_string(),
                Dim => st.dim.to_string(),
                Italic => st.italic.to_string(),
                Underline => st.underline.to_string(),
                Blink => st.blink.to_string(),
                Reverse => st.reverse.to_string(),
                Conceal => st.conceal.to_string(),
                Strikethrough => st.strikethrough.to_string(),
            }
        }
        Cursor => {
            let (r, c, _) = s.cursor();
            format!("({r},{c})")
        }
        CursorVisible => s.cursor_visible().to_string(),
        CursorShape => format!("{:?}", s.cursor_shape()).to_lowercase(),
        Title => s.title().to_string(),
        AlternateScreen => s.alternate_screen().to_string(),
        BracketedPaste => s.bracketed_paste().to_string(),
        ApplicationCursor => s.application_cursor().to_string(),
        MouseReporting => (!s.mouse_modes().is_empty()).to_string(),
    })
}

/// What the kernel reports for a probe, or `None` where it has no way to say. Each slice-3
/// capability turns a `None` here into a value, and the oracle then says whether it is right.
fn kernel_probe(s: &prova_terminal::Screen, p: Probe) -> Option<String> {
    match p {
        Text => Some(rows_text((0..usize::from(s.rows)).map(|r| s.line(r).to_string()))),
        Cell(r, c, a) => {
            let cell = s.cell(usize::from(r), usize::from(c))?;
            match a {
                Fg => Some(cell.fg.clone()),
                Bg => Some(cell.bg.clone()),
                Bold => Some(cell.bold.to_string()),
                Dim => Some(cell.dim.to_string()),
                Italic => Some(cell.italic.to_string()),
                Underline => Some(cell.underline.to_string()),
                Reverse => Some(cell.reverse.to_string()),
                Blink | Conceal | Strikethrough => None,
            }
        }
        Cursor => Some(format!("({},{})", s.cursor.0, s.cursor.1)),
        CursorVisible => Some(s.cursor_visible.to_string()),
        Title => Some(s.title.clone()),
        AlternateScreen => Some(s.alternate_screen.to_string()),
        BracketedPaste => Some(s.bracketed_paste.to_string()),
        ApplicationCursor => Some(s.application_cursor.to_string()),
        MouseReporting => Some(s.mouse_reporting.to_string()),
        CursorShape => Some(format!("{:?}", s.cursor_shape).to_lowercase()),
    }
}

fn lens_screen(f: &Fixture) -> termlens::Screen {
    let mut t = termlens::Terminal::builder()
        .size(COLS, ROWS)
        .timeout(BOUND)
        .args(["-c", f.script])
        .spawn(f.shell)
        .unwrap_or_else(|e| panic!("{}: termlens spawn: {e}", f.name));
    t.wait_stable(QUIET).unwrap_or_else(|e| panic!("{}: termlens never settled: {e}", f.name))
}

fn kernel_screen(f: &Fixture) -> prova_terminal::Screen {
    let mut spec = SpawnSpec::new(vec![f.shell.into(), "-c".into(), f.script.into()]);
    spec.cols = COLS;
    spec.rows = ROWS;
    let mut s = Session::spawn(&spec).unwrap_or_else(|e| panic!("{}: kernel spawn: {e}", f.name));
    // The host's loop, as prova's `wait_stable` runs it: settled after QUIET with no new bytes.
    let deadline = Instant::now() + BOUND;
    let mut last = s.activity().bytes;
    let mut quiet_since = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(15));
        let now = s.activity();
        if now.bytes != last {
            last = now.bytes;
            quiet_since = Instant::now();
        } else if now.ended || quiet_since.elapsed() >= QUIET {
            break;
        }
        assert!(Instant::now() < deadline, "{}: kernel never settled", f.name);
    }
    let screen = s.screen();
    s.stop();
    screen
}

/// Every disagreement in the corpus, keyed `fixture/aspect`, with what each side said.
fn disagreements() -> BTreeMap<String, (Option<String>, Option<String>)> {
    let mut out = BTreeMap::new();
    for f in FIXTURES {
        let lens = lens_screen(f);
        let kernel = kernel_screen(f);
        for &p in f.probes {
            let (k, l) = (kernel_probe(&kernel, p), lens_probe(&lens, p));
            if k != l {
                out.insert(format!("{}/{}", f.name, p.key()), (k, l));
            }
        }
    }
    out
}

fn baseline() -> BTreeSet<String> {
    BASELINE
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect()
}

#[test]
fn the_kernel_disagrees_with_termlens_only_where_the_baseline_admits() {
    let found = disagreements();
    let admitted = baseline();
    let new: Vec<String> = found
        .iter()
        .filter(|(k, _)| !admitted.contains(*k))
        .map(|(k, (kernel, lens))| format!("  {k}: kernel {kernel:?}, termlens {lens:?}"))
        .collect();
    let gone: Vec<&String> = admitted.iter().filter(|k| !found.contains_key(*k)).collect();
    assert!(
        new.is_empty() && gone.is_empty(),
        "the oracle ratchet moved ({} disagreements now, {} admitted in tests/oracle.baseline).\n\
         NEW — the kernel disagrees with termlens and the baseline does not admit it (a \
         regression to fix, or a new fixture's gap to admit deliberately):\n{}\n\
         GONE — listed but no longer disagreeing; remove them, the list only shrinks:\n{}",
        found.len(),
        admitted.len(),
        if new.is_empty() { "  (none)".to_string() } else { new.join("\n") },
        if gone.is_empty() {
            "  (none)".to_string()
        } else {
            gone.iter().map(|k| format!("  {k}")).collect::<Vec<_>>().join("\n")
        },
    );
}
