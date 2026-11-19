//! 终端默认前景色文本的轻量级日志/状态关键词着色。
//!
//! 仅作用于主屏幕可见快照中**未携带显式 ANSI 前景/背景色**的完整单词，
//! 映射为当前主题调色板的 `NamedColor` 槽位，使所有内置与自定义主题自动获得
//! 一致语义色，同时继续享受下游对比度矫正且不污染终端网格内容。

use nebula_terminal::render::{RenderSnapshot, SnapCell};
use nebula_terminal::vte::ansi::{Color, NamedColor};

const MAX_KEYWORD_LEN: usize = 12;

fn is_default_styled(cell: &SnapCell) -> bool {
    cell.fg == Color::Named(NamedColor::Foreground)
        && cell.bg == Color::Named(NamedColor::Background)
}

fn single_ascii_byte(cell: &SnapCell) -> Option<u8> {
    let bytes = cell.text.as_bytes();
    (bytes.len() == 1 && bytes[0].is_ascii()).then_some(bytes[0])
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}

fn keyword_color(raw: &[u8]) -> Option<NamedColor> {
    if raw.len() < 2 || raw.len() > MAX_KEYWORD_LEN {
        return None;
    }
    // 极短的通用英文词要求全大写，避免误染普通英文句子。
    match raw {
        b"OK" | b"PASS" => return Some(NamedColor::Green),
        _ => {},
    }
    let mut lower = [0u8; MAX_KEYWORD_LEN];
    for (dst, &src) in lower.iter_mut().zip(raw.iter()) {
        *dst = src.to_ascii_lowercase();
    }
    match &lower[..raw.len()] {
        b"error" | b"err" | b"fatal" | b"critical" | b"crit" | b"fail" | b"failed" | b"failure"
        | b"panic" | b"exception" | b"denied" | b"refused" | b"aborted" => Some(NamedColor::Red),
        b"succeed" | b"succeeded" | b"success" | b"successful" | b"successfully" | b"passed" => {
            Some(NamedColor::Green)
        },
        b"warning" | b"warn" | b"caution" | b"deprecated" => Some(NamedColor::Yellow),
        b"info" | b"notice" => Some(NamedColor::Cyan),
        b"debug" | b"trace" => Some(NamedColor::Blue),
        _ => None,
    }
}

/// 为快照中默认着色的独立状态关键词写入语义 `NamedColor`。
pub(super) fn apply_keyword_colors(snap: &mut RenderSnapshot) {
    let mut word = [0u8; MAX_KEYWORD_LEN];
    for segment in &mut snap.segments {
        if segment.wide || segment.cells.len() < 2 {
            continue;
        }
        let cells = &mut segment.cells;
        let len = cells.len();
        let mut index = 0;
        while index < len {
            let Some(byte) = single_ascii_byte(&cells[index]) else {
                index += 1;
                continue;
            };
            if !is_word_byte(byte) {
                index += 1;
                continue;
            }
            let start = index;
            let mut end = index + 1;
            while end < len
                && cells[end].col == cells[end - 1].col + 1
                && single_ascii_byte(&cells[end]).is_some_and(is_word_byte)
            {
                end += 1;
            }
            let word_len = end - start;
            if (2..=MAX_KEYWORD_LEN).contains(&word_len)
                && cells[start..end].iter().all(is_default_styled)
            {
                for (offset, cell) in cells[start..end].iter().enumerate() {
                    word[offset] = cell.text.as_bytes()[0];
                }
                if let Some(color) = keyword_color(&word[..word_len]) {
                    for cell in &mut cells[start..end] {
                        cell.fg = Color::Named(color);
                    }
                }
            }
            index = end;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nebula_terminal::event::VoidListener;
    use nebula_terminal::render::SnapshotConfig;
    use nebula_terminal::term::test::TermSize;
    use nebula_terminal::term::{Config, Term};
    use nebula_terminal::vte::ansi::{self, Rgb};

    fn snapshot_for(bytes: &[u8]) -> RenderSnapshot {
        let mut term = Term::new(Config::default(), &TermSize::new(80, 4), VoidListener);
        let mut parser: ansi::Processor = ansi::Processor::new();
        parser.advance(&mut term, bytes);
        let mut snap = RenderSnapshot::capture(&term, &SnapshotConfig { rows: 4, cols: 80 });
        apply_keyword_colors(&mut snap);
        snap
    }

    fn cell_fg_at(snap: &RenderSnapshot, row: u16, col: u16) -> Color {
        snap.segments
            .iter()
            .filter(|seg| seg.row == row)
            .flat_map(|seg| seg.cells.iter())
            .find(|cell| cell.col == col)
            .expect("cell at requested column")
            .fg
    }

    #[test]
    fn highlights_plain_status_keywords_with_theme_named_colors() {
        let snap = snapshot_for(b"ERROR: failed\r\n[INFO] Build Succeed\r\n(Warning) DEBUG OK");
        // "ERROR" -> Red, ":" -> Foreground, "failed" -> Red
        assert_eq!(cell_fg_at(&snap, 0, 0), Color::Named(NamedColor::Red));
        assert_eq!(cell_fg_at(&snap, 0, 4), Color::Named(NamedColor::Red));
        assert_eq!(cell_fg_at(&snap, 0, 5), Color::Named(NamedColor::Foreground));
        assert_eq!(cell_fg_at(&snap, 0, 7), Color::Named(NamedColor::Red));
        // "[INFO] Build Succeed"
        assert_eq!(cell_fg_at(&snap, 1, 0), Color::Named(NamedColor::Foreground));
        assert_eq!(cell_fg_at(&snap, 1, 1), Color::Named(NamedColor::Cyan));
        assert_eq!(cell_fg_at(&snap, 1, 5), Color::Named(NamedColor::Foreground));
        assert_eq!(cell_fg_at(&snap, 1, 7), Color::Named(NamedColor::Foreground));
        assert_eq!(cell_fg_at(&snap, 1, 13), Color::Named(NamedColor::Green));
        // "(Warning) DEBUG OK"
        assert_eq!(cell_fg_at(&snap, 2, 1), Color::Named(NamedColor::Yellow));
        assert_eq!(cell_fg_at(&snap, 2, 10), Color::Named(NamedColor::Blue));
        assert_eq!(cell_fg_at(&snap, 2, 16), Color::Named(NamedColor::Green));
    }

    #[test]
    fn respects_word_boundaries_and_case_rules() {
        let snap = snapshot_for(b"my_error --warn-all information ok pass");
        for col in [0, 3, 11, 20, 32, 35] {
            assert_eq!(cell_fg_at(&snap, 0, col), Color::Named(NamedColor::Foreground));
        }
    }

    #[test]
    fn preserves_explicit_ansi_foreground_and_background_colors() {
        let snap =
            snapshot_for(b"\x1b[35mERROR\x1b[0m \x1b[44mWarning\x1b[0m \x1b[38;2;1;2;3mINFO");
        // Explicit magenta foreground must stay magenta.
        assert_eq!(cell_fg_at(&snap, 0, 0), Color::Named(NamedColor::Magenta));
        // Explicit blue background leaves default foreground untouched.
        assert_eq!(cell_fg_at(&snap, 0, 6), Color::Named(NamedColor::Foreground));
        // Explicit truecolor foreground must stay truecolor.
        assert_eq!(cell_fg_at(&snap, 0, 14), Color::Spec(Rgb { r: 1, g: 2, b: 3 }));
    }

    #[test]
    fn every_builtin_theme_renders_distinct_keyword_colors_after_contrast_resolution() {
        use super::super::resolve_app_colors_into;
        use crate::display::terminal_color::TerminalColorResolver;
        use crate::gpui_shell::config::Settings;
        use nebula_terminal::term::color::Colors;

        let none = Colors::default();
        for name in nebula_settings::ThemeName::BUILTIN {
            let palette = Settings::load(name).palette;
            let mut snap = snapshot_for(b"ERROR Succeed Warning INFO");
            let mut resolver = TerminalColorResolver::default();
            resolve_app_colors_into(&mut snap, &palette, &none, &mut resolver);

            for (label, col) in [("ERROR", 0), ("Succeed", 6), ("Warning", 14), ("INFO", 22)] {
                let fg = palette.resolve(cell_fg_at(&snap, 0, col), &none, false);
                assert_ne!(fg, palette.foreground, "{name:?}: {label} matched default foreground");
                assert_ne!(fg, palette.background, "{name:?}: {label} matched background");
            }
        }
    }
}
