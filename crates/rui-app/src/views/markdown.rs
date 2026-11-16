//! A tiny markdown subset: paragraphs, lists, headings, fences, **bold**, `code` and links.

use crate::theme::Theme;
use gpui::{
    AnyElement, Div, FontWeight, HighlightStyle, Hsla, InteractiveText, IntoElement, ParentElement,
    Styled, StyledText, UnderlineStyle, div, px,
};
use std::hash::{Hash, Hasher};
use std::ops::Range;

/// A run of inline text.
#[derive(Debug, Clone, PartialEq)]
pub enum Span {
    Text(String),
    Bold(String),
    Code(String),
    Link { text: String, url: String },
}

/// One block of a markdown document.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Para(Vec<Span>),
    Bullet(Vec<Span>),
    Numbered(u32, Vec<Span>),
    Heading(u8, Vec<Span>),
    Code(String),
}

fn flush_text(out: &mut Vec<Span>, buf: &mut String) {
    if !buf.is_empty() {
        out.push(Span::Text(std::mem::take(buf)));
    }
}

/// Text up to the closing `marker`, and the rest after it.
fn split_at_marker<'a>(s: &'a str, marker: &str) -> Option<(&'a str, &'a str)> {
    let end = s.find(marker).filter(|&i| i > 0)?;
    Some((&s[..end], &s[end + marker.len()..]))
}

fn bold_span(inner: &str) -> Span {
    Span::Bold(inner.replace('`', ""))
}

fn is_http(url: &str) -> bool {
    ["http://", "https://"].iter().any(|p| url.len() > p.len() && url.starts_with(p))
}

/// `[text](url)` at the start of `s`; only http(s) urls count.
fn md_link(s: &str) -> Option<(Span, &str)> {
    let (text, after) = split_at_marker(&s[1..], "](")?;
    let (url, after) = split_at_marker(after, ")")?;
    is_http(url).then(|| (Span::Link { text: text.into(), url: url.into() }, after))
}

/// A bare `http(s)://` url at the start of `s`, minus trailing punctuation.
fn bare_url(s: &str) -> Option<(Span, &str)> {
    let end = s.find(char::is_whitespace).unwrap_or(s.len());
    let url = s[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '"', '\'']);
    is_http(url).then(|| (Span::Link { text: url.into(), url: url.into() }, &s[url.len()..]))
}

/// Parse inline `**bold**`, `` `code` `` and links. Unmatched markers stay literal.
pub fn inline(src: &str) -> Vec<Span> {
    let (mut out, mut buf, mut rest) = (Vec::new(), String::new(), src);
    while let Some(c) = rest.chars().next() {
        let found = if rest.starts_with("**") {
            split_at_marker(&rest[2..], "**").map(|(a, b)| (bold_span(a), b))
        } else if c == '`' {
            split_at_marker(&rest[1..], "`").map(|(a, b)| (Span::Code(a.to_string()), b))
        } else if c == '[' {
            md_link(rest)
        } else if c == 'h' && !buf.ends_with(|p: char| p.is_alphanumeric()) {
            bare_url(rest)
        } else {
            None
        };
        let Some((span, after)) = found else {
            buf.push(c);
            rest = &rest[c.len_utf8()..];
            continue;
        };
        flush_text(&mut out, &mut buf);
        out.push(span);
        rest = after;
    }
    flush_text(&mut out, &mut buf);
    out
}

fn numbered(line: &str) -> Option<(u32, &str)> {
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    let rest = line[digits..].strip_prefix(". ")?;
    Some((line[..digits].parse().ok()?, rest))
}

fn heading(line: &str) -> Option<(u8, &str)> {
    (1..=3u8).find_map(|n| {
        let marks = "#".repeat(n as usize);
        line.strip_prefix(&marks)?.strip_prefix(' ').map(|t| (n, t))
    })
}

fn line_block(line: &str) -> Option<Block> {
    if let Some(t) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
        return Some(Block::Bullet(inline(t.trim())));
    }
    if let Some((n, t)) = numbered(line) {
        return Some(Block::Numbered(n, inline(t.trim())));
    }
    heading(line).map(|(n, t)| Block::Heading(n, inline(t.trim())))
}

fn flush_para(blocks: &mut Vec<Block>, para: &mut Vec<String>) {
    if para.is_empty() {
        return;
    }
    blocks.push(Block::Para(inline(&para.join(" "))));
    para.clear();
}

/// Parse markdown into blocks.
pub fn parse(src: &str) -> Vec<Block> {
    let (mut blocks, mut para) = (Vec::new(), Vec::new());
    let mut lines = src.lines();
    while let Some(raw) = lines.next() {
        let line = raw.trim();
        if line.starts_with("```") {
            flush_para(&mut blocks, &mut para);
            let code: Vec<&str> = lines.by_ref().take_while(|l| !l.trim().starts_with("```")).collect();
            blocks.push(Block::Code(code.join("\n")));
        } else if line.is_empty() {
            flush_para(&mut blocks, &mut para);
        } else if let Some(b) = line_block(line) {
            flush_para(&mut blocks, &mut para);
            blocks.push(b);
        } else {
            para.push(line.to_string());
        }
    }
    flush_para(&mut blocks, &mut para);
    blocks
}

/// Highlight for inline code: a faint tint of the text color.
pub fn code_style(base: Hsla) -> HighlightStyle {
    HighlightStyle { color: Some(base), background_color: Some(base.opacity(0.10)), ..Default::default() }
}

fn bold_style(base: Hsla) -> HighlightStyle {
    HighlightStyle { color: Some(base), font_weight: Some(FontWeight::SEMIBOLD), ..Default::default() }
}

/// Underlined accent text for links.
fn link_style(color: Hsla) -> HighlightStyle {
    let underline = UnderlineStyle { thickness: px(1.), color: Some(color), wavy: false };
    HighlightStyle { color: Some(color), underline: Some(underline), ..Default::default() }
}

type Marks = Vec<(Range<usize>, HighlightStyle)>;

/// Flattened text, its highlight ranges and the (range, url) of each link.
fn flatten(spans: &[Span], base: Hsla, link: Hsla) -> (String, Marks, Vec<(Range<usize>, String)>) {
    let (mut text, mut marks, mut links) = (String::new(), Vec::new(), Vec::new());
    for span in spans {
        let start = text.len();
        let (s, style) = match span {
            Span::Text(s) => (s, None),
            Span::Bold(s) => (s, Some(bold_style(base))),
            Span::Code(s) => (s, Some(code_style(base))),
            Span::Link { text, url } => {
                links.push((start..start + text.len(), url.clone()));
                (text, Some(link_style(link)))
            }
        };
        text.push_str(s);
        marks.extend(style.map(|st| (start..text.len(), st)));
    }
    (text, marks, links)
}

/// Styled text; links are clickable and open in the browser.
fn styled(spans: &[Span], base: Hsla, th: &Theme) -> AnyElement {
    let link = if base == th.on_accent { base } else { th.accent_text };
    let (text, marks, links) = flatten(spans, base, link);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    let id = ("md-link", hasher.finish() as usize);
    let styled = StyledText::new(text).with_highlights(marks);
    if links.is_empty() {
        return styled.into_any_element();
    }
    let (ranges, urls): (Vec<_>, Vec<_>) = links.into_iter().unzip();
    InteractiveText::new(id, styled).on_click(ranges, move |i, _, cx| cx.open_url(&urls[i])).into_any_element()
}

fn marker_row(mark: String, width: f32, body: AnyElement) -> Div {
    let mark = div().flex_none().w(px(width)).child(mark);
    div().flex().items_start().child(mark).child(div().flex_1().min_w_0().child(body))
}

fn code_block(code: &str, th: &Theme) -> Div {
    let lines = code.lines().map(|l| {
        let shown = if l.is_empty() { " " } else { l };
        div().whitespace_nowrap().child(shown.to_string())
    });
    div()
        .flex()
        .flex_col()
        .overflow_hidden()
        .px(px(10.))
        .py(px(8.))
        .rounded(px(7.))
        .bg(th.well)
        .font_family("Menlo")
        .text_size(px(11.5))
        .line_height(px(17.))
        .text_color(th.fg_2)
        .children(lines)
}

fn block_el(b: &Block, base: Hsla, th: &Theme) -> Div {
    match b {
        Block::Para(s) => div().child(styled(s, base, th)),
        Block::Bullet(s) => marker_row("\u{2022}".into(), 14., styled(s, base, th)),
        Block::Numbered(n, s) => marker_row(format!("{n}."), 20., styled(s, base, th)),
        Block::Heading(_, s) => div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child(styled(s, base, th)),
        Block::Code(c) => code_block(c, th),
    }
}

/// Render markdown as a column of blocks with `base` text color and `size` text.
pub fn render(src: &str, base: Hsla, size: f32, th: &Theme) -> AnyElement {
    let blocks: Vec<Div> = parse(src).iter().map(|b| block_el(b, base, th)).collect();
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .min_w_0()
        .text_size(px(size))
        .line_height(px(size * 1.5))
        .text_color(base)
        .children(blocks)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Span {
        Span::Text(s.into())
    }

    #[test]
    fn paragraphs_join_and_split() {
        let got = parse("one\ntwo\n\nthree");
        assert_eq!(got, vec![Block::Para(vec![t("one two")]), Block::Para(vec![t("three")])]);
    }

    #[test]
    fn bullets_and_numbers() {
        let got = parse("- a\n* b\n1. c\n12. d");
        assert_eq!(got, vec![
            Block::Bullet(vec![t("a")]),
            Block::Bullet(vec![t("b")]),
            Block::Numbered(1, vec![t("c")]),
            Block::Numbered(12, vec![t("d")]),
        ]);
    }

    #[test]
    fn headings() {
        let got = parse("# A\n## B\n### C\n#### D");
        assert_eq!(got[0], Block::Heading(1, vec![t("A")]));
        assert_eq!(got[2], Block::Heading(3, vec![t("C")]));
        assert_eq!(got[3], Block::Para(vec![t("#### D")]));
    }

    #[test]
    fn fenced_code_keeps_raw_lines() {
        let got = parse("hi\n```rust\nfn a() {}\n\n- not a bullet\n```\nafter");
        assert_eq!(got, vec![
            Block::Para(vec![t("hi")]),
            Block::Code("fn a() {}\n\n- not a bullet".into()),
            Block::Para(vec![t("after")]),
        ]);
    }

    #[test]
    fn bold_and_code_inline() {
        let got = inline("use **this** and `that` now");
        assert_eq!(got, vec![
            t("use "),
            Span::Bold("this".into()),
            t(" and "),
            Span::Code("that".into()),
            t(" now"),
        ]);
    }

    #[test]
    fn nested_bold_code_has_no_markers() {
        assert_eq!(inline("**`MAX_EXPONENT`**"), vec![Span::Bold("MAX_EXPONENT".into())]);
    }

    fn link(text: &str, url: &str) -> Span {
        Span::Link { text: text.into(), url: url.into() }
    }

    #[test]
    fn markdown_links() {
        let got = inline("see [the docs](https://a.io/x?y=1) now");
        assert_eq!(got, vec![t("see "), link("the docs", "https://a.io/x?y=1"), t(" now")]);
        assert_eq!(inline("[x](ftp://a)"), vec![t("[x](ftp://a)")]);
    }

    #[test]
    fn bare_urls() {
        let got = inline("- api: http://localhost:8101, ok");
        assert_eq!(got, vec![t("- api: "), link("http://localhost:8101", "http://localhost:8101"), t(", ok")]);
        assert_eq!(inline("go to https://a.io."), vec![t("go to "), link("https://a.io", "https://a.io"), t(".")]);
        assert_eq!(inline("xhttp://a.io"), vec![t("xhttp://a.io")]);
        assert_eq!(inline("http:// alone"), vec![t("http:// alone")]);
    }

    #[test]
    fn list_item_keeps_link() {
        let got = parse("- api: http://localhost:8101");
        let Block::Bullet(spans) = &got[0] else { panic!() };
        assert_eq!(spans[1], link("http://localhost:8101", "http://localhost:8101"));
    }

    #[test]
    fn unmatched_markers_stay_literal() {
        assert_eq!(inline("a ** b"), vec![t("a ** b")]);
        assert_eq!(inline("a ` b"), vec![t("a ` b")]);
        assert_eq!(inline("2 * 3"), vec![t("2 * 3")]);
    }
}
