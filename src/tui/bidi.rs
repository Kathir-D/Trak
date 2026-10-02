//! Right-to-left text next to the layout (TODO 11.6).
//!
//! Trak draws cells left to right and leaves bidi to the terminal, which is
//! right for the words: a Hebrew or Arabic title should read the way its script
//! does. But a terminal that reorders (cmux does) works on the whole row, and
//! digits and spaces after an RTL run join it. A header of "title, padding,
//! 08:27" came out as "08:27, padding, eltit": the clock jumped left and the
//! title right. That is the Unicode bidi algorithm doing its job (numbers next
//! to RTL text belong to it), so the fix is to give it a strong left-to-right
//! character between the two: [`fence`] puts one in the first blank cell after
//! every RTL run.
//!
//! The character is U+2800 BRAILLE PATTERN BLANK: bidi class L, one column,
//! and it draws nothing. The obvious LEFT-TO-RIGHT MARK does not work: it has
//! no width, ratatui drops zero-width graphemes, and appended to a letter's
//! cell instead it reached cmux and was ignored (seen 2026-10-02).

use ratatui::buffer::Buffer;

const FENCE: &str = "\u{2800}";

/// Whether `c` is a strong right-to-left letter: Hebrew, Arabic, Syriac,
/// Thaana, N'Ko and the rest of U+0590..U+08FF, plus the presentation forms.
pub fn is_rtl(c: char) -> bool {
    matches!(c, '\u{0590}'..='\u{08FF}' | '\u{FB1D}'..='\u{FDFF}' | '\u{FE70}'..='\u{FEFF}')
}

fn strong_ltr(symbol: &str) -> bool {
    symbol
        .chars()
        .any(|c| c.is_alphanumeric() && !is_rtl(c) && !c.is_numeric())
}

/// A cell that ends a run of text: a border or a rule, which are layout rather
/// than words.
fn is_layout(symbol: &str) -> bool {
    symbol
        .chars()
        .any(|c| matches!(c, '\u{2500}'..='\u{259F}' | '\u{25A0}'..='\u{25FF}'))
}

/// Fence every right-to-left run on every row off from what follows it.
///
/// A run ends at a strong left-to-right letter, at a border, at two blank
/// cells in a row (the padding between a title and whatever is beside it) or
/// at the end of the row. Punctuation and single spaces inside it do not end
/// it, so "artist — title" in two RTL scripts stays one run, as it should.
pub fn fence(buf: &mut Buffer) {
    let area = buf.area;
    for y in area.top()..area.bottom() {
        // The last cell of the run so far, if the row is inside one.
        let mut last_rtl: Option<u16> = None;
        let mut blanks = 0;
        for x in area.left()..area.right() {
            let symbol = buf[(x, y)].symbol();
            if symbol.chars().any(is_rtl) {
                last_rtl = Some(x);
                blanks = 0;
                continue;
            }
            let Some(end) = last_rtl else { continue };
            let blank = symbol.trim().is_empty();
            blanks = if blank { blanks + 1 } else { 0 };
            if symbol == FENCE || strong_ltr(symbol) || is_layout(symbol) || blanks >= 2 {
                mark(buf, end, y);
                last_rtl = None;
                blanks = 0;
            }
        }
        if let Some(end) = last_rtl {
            mark(buf, end, y);
        }
    }
}

/// Put the fence in the first blank cell after `end`. A run with no blank
/// after it before the next RTL text, or the row's end, needs none: nothing
/// follows it that could be pulled in. Cells marked `skip` belong to an image
/// and are never touched.
fn mark(buf: &mut Buffer, end: u16, y: u16) {
    for x in end + 1..buf.area.right() {
        let cell = &buf[(x, y)];
        if cell.symbol() == FENCE || cell.symbol().chars().any(is_rtl) {
            return;
        }
        if cell.symbol() == " " && !cell.skip {
            buf[(x, y)].set_symbol(FENCE);
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    fn row(text: &str) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, text.chars().count() as u16, 1));
        buf.set_string(0, 0, text, ratatui::style::Style::default());
        buf
    }

    /// Each fence, as the text just before it, so a test reads as "fenced
    /// after this".
    fn marked(buf: &Buffer) -> Vec<String> {
        (1..buf.area.width)
            .filter(|&x| buf[(x, 0)].symbol() == FENCE)
            .map(|x| buf[(x - 1, 0)].symbol().to_string())
            .collect()
    }

    /// The header case that broke: the title is fenced before the padding, so
    /// the clock stays at the right.
    #[test]
    fn a_title_is_fenced_off_from_the_clock() {
        let mut buf = row("Trak  שיר ארוך        08:27");
        fence(&mut buf);
        assert_eq!(marked(&buf), ["ך"]);
    }

    /// Two scripts joined by a dash are one run; the mark goes after both.
    #[test]
    fn a_run_holds_across_single_spaces_and_punctuation() {
        let mut buf = row("▶ فيروز — שיר ארוך  │");
        fence(&mut buf);
        assert_eq!(marked(&buf), ["ך"]);
    }

    #[test]
    fn a_border_or_latin_ends_a_run_and_the_row_end_does_too() {
        let mut buf = row("שלום│ abc שלום x שלום");
        fence(&mut buf);
        // After the border the first blank is the next one; at the row's end
        // there is nothing to fence off.
        assert_eq!(marked(&buf), ["│", "ם"]);
    }

    #[test]
    fn latin_only_rows_are_untouched() {
        let mut buf = row("Jane Remover — Census Designated  08:27");
        let before = buf.clone();
        fence(&mut buf);
        assert_eq!(buf, before);
    }

    /// The fence is one column, like the space it replaces, and fencing an
    /// already fenced frame changes nothing.
    #[test]
    fn the_fence_is_one_column_and_idempotent() {
        let mut buf = row("שיר   08:30");
        fence(&mut buf);
        let once = buf.clone();
        fence(&mut buf);
        assert_eq!(buf, once);
        assert_eq!(ratatui::text::Span::raw(FENCE).width(), 1);
        assert_eq!(buf[(3, 0)].symbol(), FENCE);
    }

    /// An image's cells are not text and are never written to.
    #[test]
    fn an_image_cell_is_never_the_fence() {
        let mut buf = row("שיר  x");
        buf[(3, 0)].set_skip(true);
        fence(&mut buf);
        assert_eq!(buf[(3, 0)].symbol(), " ");
        assert_eq!(buf[(4, 0)].symbol(), FENCE);
    }
}
