//! Inline run normalization and rendering.

use crate::model::{ImageSource, Inline, LinkTarget, Style, inlines_are_empty};
use crate::render::markdown::Ctx;
use crate::render::markdown::escape::{
    EscapeOpts, InlineContext, backtick_fence, escape_text, escape_url_as_text, format_url,
};
use std::borrow::Cow;
use std::fmt::Write as _;

pub(crate) enum Norm<'a> {
    Text { text: Cow<'a, str>, style: Style },
    Link { content: &'a [Inline], target: &'a LinkTarget },
    Image { alt: &'a str, source: &'a ImageSource },
    Anchor(&'a str),
    NoteRef(&'a str),
    LineBreak,
}

/// Single-pass normalization: drops empty runs, strips styling from
/// whitespace-only runs, merges adjacent same-style runs, and re-joins styled
/// runs split only by whitespace (`**a** **b**` -> `**a b**`). Runs borrow
/// from the source inlines; text is owned only where runs actually merge.
/// Untargeted anchors drop out here: they render as nothing, so leaving them
/// in would part runs that belong together.
pub(crate) fn normalize<'a>(inlines: &'a [Inline], rc: &Ctx) -> Vec<Norm<'a>> {
    let mut out: Vec<Norm<'a>> = Vec::new();
    for inline in inlines {
        match inline {
            Inline::Text { text, style } => {
                if text.is_empty() {
                    continue;
                }
                let style = if text.trim().is_empty() { Style::PLAIN } else { *style };
                if let Some(Norm::Text { text: prev, style: prev_style }) = out.last_mut()
                    && *prev_style == style
                {
                    prev.to_mut().push_str(text);
                    continue;
                }
                // Bridge: [styled S][ws plain][incoming styled S] merges into one run.
                if style != Style::PLAIN
                    && !style.code
                    && out.len() >= 2
                    && matches!(&out[out.len() - 1],
                        Norm::Text { text: ws, style: s } if *s == Style::PLAIN && ws.trim().is_empty())
                    && matches!(&out[out.len() - 2],
                        Norm::Text { style: s, .. } if *s == style)
                {
                    let Some(Norm::Text { text: ws, .. }) = out.pop() else { unreachable!() };
                    let Some(Norm::Text { text: prev, .. }) = out.last_mut() else {
                        unreachable!()
                    };
                    let prev = prev.to_mut();
                    prev.push_str(&ws);
                    prev.push_str(text);
                    continue;
                }
                out.push(Norm::Text { text: Cow::Borrowed(text.as_str()), style });
            }
            Inline::Link { content, target } => {
                if target.is_empty() {
                    // No usable destination: keep the content as plain inlines.
                    if !inlines_are_empty(content) {
                        out.extend(normalize(content, rc));
                    }
                    continue;
                }
                out.push(Norm::Link { content, target });
            }
            Inline::Image { alt, source } => out.push(Norm::Image { alt, source }),
            Inline::Anchor(id) if rc.anchors.html_id(id).is_none() => continue,
            Inline::Anchor(id) => out.push(Norm::Anchor(id)),
            Inline::NoteRef(id) => out.push(Norm::NoteRef(id)),
            Inline::LineBreak => out.push(Norm::LineBreak),
        }
    }
    out
}

pub(crate) fn render_inlines(inlines: &[Inline], ctx: InlineContext, rc: &Ctx) -> String {
    render_inlines_mode(inlines, ctx, false, rc)
}

fn render_inlines_mode(inlines: &[Inline], ctx: InlineContext, in_label: bool, rc: &Ctx) -> String {
    let runs = normalize(inlines, rc);
    let mut out = String::new();
    for (idx, run) in runs.iter().enumerate() {
        match run {
            Norm::Text { text, style } => {
                let next_active = matches!(
                    runs.get(idx + 1),
                    Some(Norm::Link { .. } | Norm::Image { .. } | Norm::NoteRef(_))
                ) || matches!(
                    runs.get(idx + 1),
                    Some(Norm::Text { style, .. }) if *style != Style::PLAIN
                );
                render_text_run(text, *style, ctx, next_active, in_label, &mut out)
            }
            Norm::NoteRef(id) => {
                if let Some(num) = rc.nums.get(*id) {
                    let _ = write!(out, "[^{num}]");
                }
            }
            Norm::Link { content, target } => render_link(content, target, ctx, rc, &mut out),
            Norm::Image { alt, source } => render_image(alt, source, ctx, in_label, &mut out),
            Norm::Anchor(id) => {
                if let Some(html_id) = rc.anchors.html_id(id) {
                    let _ = write!(out, "<a id=\"{html_id}\"></a>");
                }
            }
            Norm::LineBreak => match ctx {
                InlineContext::Block => out.push_str("\\\n"),
                InlineContext::Heading => out.push(' '),
                InlineContext::TableCell => out.push('\n'),
            },
        }
    }
    out
}

fn render_link(
    content: &[Inline],
    target: &LinkTarget,
    ctx: InlineContext,
    rc: &Ctx,
    out: &mut String,
) {
    let label = render_inlines_mode(content, ctx, true, rc);
    let url = match target {
        LinkTarget::External(url) | LinkTarget::Relative(url) => url.clone(),
        LinkTarget::Anchor(id) => match rc.anchors.fragment(id) {
            Some(fragment) => format!("#{fragment}"),
            None => {
                // Target exists nowhere in the document: degrade to plain text.
                log::debug!("unresolved internal link target: {id}");
                out.push_str(&render_inlines_mode(content, ctx, false, rc));
                return;
            }
        },
    };
    // Emptiness is tested on the trimmed label, but the rendered label keeps
    // its source-significant edge spaces.
    if label.trim().is_empty() {
        if matches!(target, LinkTarget::Anchor(_)) {
            return;
        }
        let _ = write!(out, "[{}]({})", escape_url_as_text(&url, ctx), format_url(&url));
    } else {
        let _ = write!(out, "[{}]({})", label, format_url(&url));
    }
}

fn render_image(
    alt: &str,
    source: &ImageSource,
    ctx: InlineContext,
    in_label: bool,
    out: &mut String,
) {
    match source {
        ImageSource::External(url) => {
            let alt =
                escape_text(alt.trim(), ctx, EscapeOpts { in_label: true, ..Default::default() });
            let _ = write!(out, "![{}]({})", alt, format_url(url));
        }
        // Embedded assets render as their alt text: Markdown cannot embed
        // bytes, and the bytes stay available in `Document::assets`. A
        // source-less image has only its alt text to offer.
        ImageSource::Asset(_) | ImageSource::Unavailable => {
            if !alt.trim().is_empty() {
                out.push_str(&escape_text(
                    alt.trim(),
                    ctx,
                    EscapeOpts { in_label, ..Default::default() },
                ));
            }
        }
    }
}

/// Emit a styled run, moving edge whitespace outside the delimiters.
fn render_text_run(
    text: &str,
    style: Style,
    ctx: InlineContext,
    trailing_active: bool,
    in_label: bool,
    out: &mut String,
) {
    if style == Style::PLAIN {
        let at_line_start = out.is_empty() || out.ends_with('\n');
        out.push_str(&escape_text(
            text,
            ctx,
            EscapeOpts { at_line_start, trailing_active, in_label, ..Default::default() },
        ));
        return;
    }
    let core_start = text.len() - text.trim_start().len();
    let core_end = text.trim_end().len();
    let (lead, core, trail) = (&text[..core_start], &text[core_start..core_end], &text[core_end..]);
    if !lead.is_empty() {
        out.push_str(lead);
    }
    if !core.is_empty() {
        if style.code {
            push_code_span(core, ctx, out);
        } else {
            let mut open = String::new();
            if style.strike {
                open.push_str("~~");
            }
            if style.bold {
                open.push_str("**");
            }
            if style.italic {
                open.push('*');
            }
            let close: String = open.chars().rev().collect();
            out.push_str(&open);
            out.push_str(&escape_text(
                core,
                ctx,
                EscapeOpts { styled: true, in_label, ..Default::default() },
            ));
            out.push_str(&close);
        }
    }
    if !trail.is_empty() {
        out.push_str(trail);
    }
}

pub(crate) fn push_code_span(text: &str, ctx: InlineContext, out: &mut String) {
    let text = text.replace('\n', " ");
    let fence = backtick_fence(&text, 1);
    let pad = if text.starts_with('`') || text.ends_with('`') { " " } else { "" };
    // A row is split into cells before any code span is parsed, so a pipe is
    // syntax here even though everything else between the fences is literal.
    let text = match ctx {
        InlineContext::TableCell => text.replace('|', "\\|"),
        _ => text,
    };
    let _ = write!(out, "{fence}{pad}{text}{pad}{fence}");
}
