//! The document the engine scrolls through.
//!
//! Real text: the corpus in `assets/` is parsed into paragraphs, wrapped to the
//! column width, and flattened into the visual lines the engine walks. Wrapping
//! needs text measurement, which comes from the window, so the app builds the
//! document on its first frame — see `CoolScroll::build_document`.
//!
//! A line also remembers how wide each of its words is. That costs nothing —
//! wrapping measures every word anyway — and it is what lets the same document
//! be drawn as skeleton bars instead of text.

use gpui::SharedString;

/// How a line is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// A chapter or book heading.
    Heading,
    /// Running prose.
    Body,
}

impl Style {
    /// Font size, in unscaled document pixels.
    pub const fn font_size(self) -> f32 {
        match self {
            Style::Heading => HEADING_SIZE,
            Style::Body => BODY_SIZE,
        }
    }

    /// The height of the line's box: the font size plus its leading. Used to
    /// space the document and to centre the text on its line.
    pub const fn height(self) -> f32 {
        match self {
            Style::Heading => HEADING_HEIGHT,
            Style::Body => BODY_HEIGHT,
        }
    }
}

/// One line of the document, already wrapped to the column.
#[derive(Debug, Clone)]
pub struct Line {
    pub text: SharedString,
    pub style: Style,
    /// Leading indent, applied before the first word.
    pub indent: f32,
    /// How wide each of the line's words is, in unscaled document pixels.
    pub words: Vec<f32>,
    /// The width of the space between them, which is part of the line as much
    /// as the words are: a line of bars only covers the same width as the line
    /// of text if the gaps between the bars are the gaps between the words.
    pub gap: f32,
}

impl Line {
    /// The height of this line, before any scaling.
    pub fn height(&self) -> f32 {
        self.style.height()
    }
}

/// Distance between line anchors when nothing is distorting them.
pub const BASE_SPACING: f32 = 30.0;
/// The column width the document is wrapped to.
pub const CONTENT_WIDTH: f32 = 800.0;
/// Leading indent on the first line of a paragraph.
pub const INDENT: f32 = 30.0;
/// How many lines to build. Wrapping measures each word, so the demo takes a
/// long opening of the book rather than all 25,000 lines of it: raise this for
/// more, at a cost that is linear in the text.
pub const MAX_LINES: usize = 6000;
/// Font sizes and line heights, in unscaled document pixels.
const HEADING_SIZE: f32 = 20.0;
const HEADING_HEIGHT: f32 = 24.0;
const BODY_SIZE: f32 = 14.0;
const BODY_HEIGHT: f32 = 18.0;
/// Words a short line may open with to count as a heading.
const HEADING_WORDS: [&str; 6] = [
    "CHAPTER",
    "PART",
    "BOOK",
    "EPILOGUE",
    "APPENDIX",
    "INTRODUCTION",
];

/// The scrollable length of a document, in unscaled pixels.
pub fn content_height(lines: &[Line]) -> f32 {
    lines.len() as f32 * BASE_SPACING
}

/// Parse the corpus into paragraphs: blank lines separate them, and short lines
/// that open with one of [`HEADING_WORDS`] are headings.
pub fn paragraphs(text: &str) -> Vec<(Style, String)> {
    // The corpus starts with a byte order mark and uses CRLF line endings.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);

    let mut paragraphs = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        paragraphs.push((style_of(line), line.to_owned()));
    }
    paragraphs
}

fn style_of(line: &str) -> Style {
    let looks_like_a_heading =
        line.len() <= 40 && HEADING_WORDS.iter().any(|word| line.starts_with(word));
    if looks_like_a_heading {
        Style::Heading
    } else {
        Style::Body
    }
}

/// A paragraph line that has been wrapped: its text, and the width of each of
/// its words.
pub struct Wrapped {
    pub text: String,
    pub words: Vec<f32>,
}

/// Wrap `paragraph` to `first_width` for its opening line and `rest_width` after
/// that, asking `measure` for the width of a word. Runs of whitespace collapse,
/// as they do on screen.
pub fn wrap(
    paragraph: &str,
    first_width: f32,
    rest_width: f32,
    measure: &mut impl FnMut(&str) -> f32,
) -> Vec<Wrapped> {
    let space = measure(" ");
    let mut lines: Vec<Wrapped> = Vec::new();
    let mut current = String::new();
    let mut words: Vec<f32> = Vec::new();
    let mut width = 0.0;

    for word in paragraph.split_whitespace() {
        let word_width = measure(word);
        // A line is indented only when it is the first of its paragraph, which
        // costs it that much width.
        let limit = if lines.is_empty() {
            first_width
        } else {
            rest_width
        };

        if !current.is_empty() && width + space + word_width > limit {
            lines.push(Wrapped {
                text: std::mem::take(&mut current),
                words: std::mem::take(&mut words),
            });
            width = 0.0;
        }
        if !current.is_empty() {
            current.push(' ');
            width += space;
        }
        current.push_str(word);
        words.push(word_width);
        width += word_width;
    }

    if !current.is_empty() {
        lines.push(Wrapped {
            text: current,
            words,
        });
    }
    lines
}

/// Build the document: parse, wrap to the column, and stop at `max_lines`.
pub fn build(
    text: &str,
    max_lines: usize,
    measure: &mut impl FnMut(&str, Style) -> f32,
) -> Vec<Line> {
    let mut lines = Vec::new();

    for (style, paragraph) in paragraphs(text) {
        if lines.len() >= max_lines {
            break;
        }

        let gap = measure(" ", style);

        if style == Style::Heading {
            lines.push(Line {
                words: paragraph
                    .split_whitespace()
                    .map(|word| measure(word, style))
                    .collect(),
                text: paragraph.into(),
                style,
                indent: 0.0,
                gap,
            });
            continue;
        }

        let mut measure_style = |word: &str| measure(word, style);
        let wrapped = wrap(
            &paragraph,
            CONTENT_WIDTH - INDENT,
            CONTENT_WIDTH,
            &mut measure_style,
        );
        for (index, wrapped) in wrapped.into_iter().enumerate() {
            if lines.len() >= max_lines {
                break;
            }
            lines.push(Line {
                text: wrapped.text.into(),
                style,
                indent: if index == 0 { INDENT } else { 0.0 },
                words: wrapped.words,
                gap,
            });
        }
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A measuring stick where every character is 40px wide, so a few words fill
    /// the column and wrapping is easy to see without a text system.
    fn measure(text: &str, _style: Style) -> f32 {
        text.chars().count() as f32 * 40.0
    }

    fn wrapped(text: &str, lines: usize) -> Vec<Line> {
        build(text, lines, &mut measure)
    }

    #[test]
    fn paragraphs_split_on_blank_lines_and_spot_headings() {
        let corpus = "\u{feff}CHAPTER I\r\n\r\nIt was in July, 1805.\r\n\r\nCHAPTER II\r\n\r\nThe fete began.\r\n";
        let paragraphs = paragraphs(corpus);

        assert_eq!(
            paragraphs,
            vec![
                (Style::Heading, "CHAPTER I".to_owned()),
                (Style::Body, "It was in July, 1805.".to_owned()),
                (Style::Heading, "CHAPTER II".to_owned()),
                (Style::Body, "The fete began.".to_owned()),
            ],
            "the byte order mark, the CRs and the blank lines should all be gone"
        );
    }

    #[test]
    fn a_long_paragraph_is_wrapped_and_only_its_first_line_is_indented() {
        let text = "aaaaaaaaaa bbbbbbbbbb cccccccccc dddddddddd eeeeeeeeee ffffffffff";
        let lines = wrapped(text, 10);

        assert!(lines.len() > 1, "the paragraph should have been wrapped");
        assert_eq!(lines[0].indent, INDENT);
        assert!(
            lines[1..].iter().all(|line| line.indent == 0.0),
            "continuation lines start at the margin"
        );
        assert!(
            lines.iter().all(|line| !line.text.starts_with(' ')),
            "wrapping should not leave stray leading spaces"
        );
    }

    #[test]
    fn wrapping_reflows_the_text_without_losing_words() {
        let text = "one two three four five six seven eight nine ten";
        let lines = wrapped(text, 4);

        let rejoined: Vec<&str> = lines.iter().flat_map(|line| line.text.split(' ')).collect();
        assert_eq!(
            rejoined,
            vec![
                "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten"
            ]
        );
    }

    #[test]
    fn a_document_stops_at_the_line_cap() {
        let text = "one two three four five six seven eight nine ten";
        let lines = wrapped(text, 2);

        assert_eq!(lines.len(), 2, "the cap should hold");
    }

    #[test]
    fn a_line_remembers_where_each_of_its_words_is() {
        let lines = wrapped("aaaa bbbb cccc", 10);

        assert_eq!(lines.len(), 1, "the line fits the column");
        assert_eq!(lines[0].text, "aaaa bbbb cccc");
        // The stick is 40px a character, so a four-letter word is 160 wide and
        // the space between two of them is 40.
        assert_eq!(lines[0].words, vec![160.0, 160.0, 160.0]);
        assert_eq!(lines[0].gap, 40.0);
    }

    #[test]
    fn the_words_of_a_wrapped_line_cover_exactly_its_width() {
        let lines = wrapped("aaaa bbbb cccc dddd eeee ffff", 10);
        assert!(lines.len() > 1, "this paragraph should have wrapped");

        let counted: usize = lines.iter().map(|line| line.words.len()).sum();
        assert_eq!(counted, 6, "every word belongs to exactly one line");

        // Which is the property the skeleton mode leans on: bars as wide as the
        // words, with the line's own word spaces between them, come to the same
        // width as the text they stand in for.
        for line in &lines {
            let spaces = (line.words.len() - 1) as f32 * line.gap;
            let width = line.words.iter().sum::<f32>() + spaces;
            assert!(
                width <= CONTENT_WIDTH,
                "a line of bars must fit the column: {width}"
            );
            assert_eq!(
                line.words.len(),
                line.text.split(' ').count(),
                "one bar per word: {:?}",
                line.text
            );
        }
    }
}
