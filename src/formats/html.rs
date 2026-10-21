//! Standalone HTML: browser-grade HTML5 parsing followed by the shared
//! semantic HTML -> document-model frontend used by EPUB.

use crate::error::ConvertError;
use crate::model::{AnchorId, Document, ImageSource, LinkTarget};
use crate::package::limits;
use crate::package::xml::{Attr, Element, Node};
use crate::shared::html::{HtmlCtx, Stylesheet};
use crate::shared::uri::is_absolute_uri;
use encoding_rs::{Encoding, UTF_16BE, UTF_16LE, WINDOWS_1252};
use html5ever::LocalName;
use html5ever::tendril::StrTendril;
use html5ever::tokenizer::states::{Rawtext, Rcdata, ScriptData};
use html5ever::tokenizer::{
    BufferQueue, EndTag, StartTag, TagToken, Token, TokenSink, TokenSinkResult, Tokenizer,
};
use scraper::{ElementRef, Html, Node as HtmlNode};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Parse a standalone HTML document into anydoc's document model.
///
/// HTML5 tree construction is delegated to `scraper`/`html5ever`; the
/// resulting DOM is adapted into the small namespace-aware tree already used
/// by `shared::html`, so HTML and EPUB keep one semantic conversion path.
pub fn parse(bytes: &[u8]) -> Result<Document, ConvertError> {
    if bytes.len() as u64 > limits::MAX_ENTRY_BYTES {
        return Err(ConvertError::ResourceLimit {
            limit: "max_entry_bytes",
            detail: format!(
                "HTML input is {} bytes; maximum is {}",
                bytes.len(),
                limits::MAX_ENTRY_BYTES
            ),
        });
    }

    let text = decode_html(bytes);
    preflight_html_complexity(&text)?;

    let parsed = Html::parse_document(&text);
    let root = parsed.root_element();

    let mut css = Stylesheet::default();
    for style in root.descendent_elements().filter(|e| e.value().name() == "style") {
        css.add(&style.text().collect::<String>());
    }

    let body = match root.descendent_elements().find(|e| e.value().name() == "body") {
        Some(body) => body,
        None if root.descendent_elements().any(|e| e.value().name() == "frameset") => {
            return Ok(Document::default());
        }
        None => {
            return Err(ConvertError::malformed("HTML parser produced no body element"));
        }
    };

    let mut node_count = 0usize;
    let body = adapt_element(body, 1, &mut node_count)?;
    let blocks = crate::shared::html::to_blocks(&body, &css, &StandaloneCtx)?;

    Ok(Document { blocks, ..Document::default() })
}

fn decode_html(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return UTF_16LE.decode(rest).0.into_owned();
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return UTF_16BE.decode(rest).0.into_owned();
    }
    if let Some(encoding) = sniff_meta_charset(bytes) {
        return encoding.decode(bytes).0.into_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(_) => WINDOWS_1252.decode(bytes).0.into_owned(),
    }
}

fn sniff_meta_charset(bytes: &[u8]) -> Option<&'static Encoding> {
    const SNIFF_BYTES: usize = 64 * 1024;
    let prefix = String::from_utf8_lossy(&bytes[..bytes.len().min(SNIFF_BYTES)]);
    let parsed = Html::parse_document(prefix.as_ref());
    let root = parsed.root_element();

    for meta in root.descendent_elements().filter(|element| element.value().name() == "meta") {
        if let Some(label) = meta.value().attr("charset")
            && let Some(encoding) = Encoding::for_label(label.trim().as_bytes())
        {
            return Some(encoding);
        }

        let is_content_type = meta
            .value()
            .attr("http-equiv")
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("content-type"));
        if is_content_type
            && let Some(content) = meta.value().attr("content")
            && let Some(label) = content_type_charset(content)
            && let Some(encoding) = Encoding::for_label(label.as_bytes())
        {
            return Some(encoding);
        }
    }
    None
}

fn content_type_charset(content: &str) -> Option<&str> {
    let mut start = 0usize;
    let mut quote = None;
    for (index, ch) in content.char_indices() {
        match (quote, ch) {
            (Some(active), current) if current == active => quote = None,
            (None, '\'' | '"') => quote = Some(ch),
            (None, ';') => {
                if let Some(label) = charset_parameter(&content[start..index]) {
                    return Some(label);
                }
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    charset_parameter(&content[start..])
}

fn charset_parameter(parameter: &str) -> Option<&str> {
    let (name, value) = parameter.split_once('=')?;
    if !name.trim().eq_ignore_ascii_case("charset") {
        return None;
    }
    let label = value.trim().trim_matches(|c| c == '\'' || c == '"').trim();
    (!label.is_empty()).then_some(label)
}

#[derive(Default)]
struct HtmlComplexitySink {
    node_count: Cell<usize>,
    open_elements: RefCell<Vec<LocalName>>,
    node_limit_exceeded: Cell<bool>,
    depth_limit_exceeded: Cell<bool>,
}

impl HtmlComplexitySink {
    fn bump_node(&self) {
        let count = self.node_count.get().saturating_add(1);
        self.node_count.set(count);
        if count > limits::MAX_XML_NODES {
            self.node_limit_exceeded.set(true);
        }
    }

    fn close_implied(&self, name: &str) {
        let mut open = self.open_elements.borrow_mut();
        if !in_foreign_content(&open) {
            // Void tags are never pushed, so the ignore-token result only
            // matters for callers that would push (push_element).
            let _ignore = close_implied_before_start(&mut open, name);
        }
    }

    fn push_element(&self, name: &LocalName) {
        let mut open = self.open_elements.borrow_mut();
        // html5ever ignores duplicate <html>/<body> start tags once those
        // wrappers exist; pushing them again would move the depth baseline
        // used below and undercount every later descendant. Inside foreign
        // content these names are ordinary foreign elements (html is not a
        // breakout tag), so the suppression must not apply there.
        if matches!(name.as_ref(), "html" | "body")
            && !in_foreign_content(&open)
            && open.iter().any(|candidate| candidate == name)
        {
            return;
        }
        // HTML implied-end-tag rules only apply in HTML content; inside
        // foreign content names like option/tr are ordinary elements and
        // closing them here would undercount real nesting.
        if !in_foreign_content(&open) && close_implied_before_start(&mut open, name.as_ref()) {
            // html5ever ignores table-family start tags outside a table in
            // body context; pushing them would grow the modeled stack where
            // the parser does not, and a later implied close would truncate
            // the real nesting opened between the stray cells.
            return;
        }
        open.push(name.clone());
        // Count depth the way the post-parse walk will: adapt_element treats
        // the body element as depth 1, and html5ever auto-inserts html/body
        // wrappers that may never appear as tokens.
        let wrapper = open
            .iter()
            .rposition(|candidate| candidate.as_ref() == "body")
            .or_else(|| open.iter().rposition(|candidate| candidate.as_ref() == "html"));
        let depth = match wrapper {
            Some(index) => open.len() - index,
            None => open.len() + 1,
        };
        if depth > limits::MAX_XML_DEPTH {
            self.depth_limit_exceeded.set(true);
        }
    }

    fn close_element(&self, name: &LocalName) {
        let mut open = self.open_elements.borrow_mut();
        let name = name.as_ref();
        // html5ever routes an end tag through the foreign-content walk
        // whenever the adjusted current node is foreign (integration points
        // included). That walk truncates on a name match inside the foreign
        // region (any namespace) and, failing that, reprocesses the token in
        // the HTML insertion mode once it reaches an HTML element below the
        // root. The unified scope walk at the bottom models the reprocessed
        // rules. body/html and option/optgroup need the region distinction
        // because their in-body rules never pop (mode switch / current-node
        // check), while their foreign-region matches really do pop.
        let foreign_root = in_foreign_content(&open).then(|| foreign_root_index(&open)).flatten();
        if matches!(name, "body" | "html") {
            if let Some(root) = foreign_root
                && let Some((position, _)) = open
                    .iter()
                    .enumerate()
                    .rev()
                    .take_while(|(index, _)| *index >= root)
                    .find(|(_, candidate)| candidate.as_ref() == name)
            {
                open.truncate(position);
            }
            // In HTML content these end tags only switch insertion modes
            // (after body / after after body); later start tags keep nesting
            // in the existing tree, so the modeled stack must stay untouched.
            return;
        }
        if matches!(name, "option" | "optgroup") {
            if let Some(root) = foreign_root {
                if let Some((position, _)) = open
                    .iter()
                    .enumerate()
                    .rev()
                    .take_while(|(index, _)| *index >= root)
                    .find(|(_, candidate)| candidate.as_ref() == name)
                {
                    open.truncate(position);
                }
                return;
            }
            // In HTML content html5ever checks only the current node for
            // these end tags; searching deeper would truncate real nesting
            // the parser keeps open.
            if name == "optgroup" && open.last().is_some_and(|top| top.as_ref() == "option") {
                open.pop();
            }
            if open.last().is_some_and(|top| top.as_ref() == name) {
                open.pop();
            }
            return;
        }
        // Unified walk: models both the foreign-phase name-match pop (bare
        // svg/math roots do not stop it) and the reprocessed in-body scope
        // rules. html5ever ignores an end tag whose element is not in scope;
        // the search stops at markers that depend on the end tag (table scope
        // for the table family, list-item scope for li, button scope for
        // p/dd/dt, ruby scope for rt/rp, the special category for any other
        // end tag). Truncating on an out-of-scope match would pop real
        // nesting and undercount depth; ignoring an in-scope match would
        // over-count and can falsely reject valid documents.
        for (position, candidate) in open.iter().enumerate().rev() {
            if candidate.as_ref() == name {
                open.truncate(position);
                return;
            }
            if is_end_tag_scope_marker(name, candidate.as_ref()) {
                return;
            }
        }
    }

    /// Pop through the nearest svg/math ancestor when an HTML breakout tag
    /// arrives, mirroring html5ever's foreign-content exit. An HTML
    /// integration point above the foreign root already restores HTML
    /// semantics, so the stack is left untouched in that case.
    fn pop_foreign_breakout(&self) {
        let mut open = self.open_elements.borrow_mut();
        // html5ever pops until the current node is an HTML integration point
        // or an HTML element, so nested foreign roots (svg inside math, etc.)
        // all leave the stack: truncate at the outermost reachable root.
        if let Some(index) = foreign_root_index(&open) {
            open.truncate(index);
        }
    }
}

impl TokenSink for HtmlComplexitySink {
    type Handle = ();

    fn process_token(&self, token: Token, _line_number: u64) -> TokenSinkResult<Self::Handle> {
        match token {
            TagToken(tag) => match tag.kind {
                StartTag => {
                    self.bump_node();
                    let name = tag.name.as_ref();
                    // html5ever leaves foreign content when an HTML breakout
                    // tag arrives. Model that before classifying self-closing
                    // tags, otherwise a stale svg/math entry makes later HTML
                    // tags look foreign and their self-closing forms skip the
                    // depth accounting.
                    if is_foreign_content_html_breakout(name) {
                        self.pop_foreign_breakout();
                    }
                    let honor_self_closing = tag.self_closing
                        && html5_self_closing_is_honored(&self.open_elements.borrow(), name);
                    // Raw-text tokenizer states are an HTML-content decision:
                    // it must be taken before the new element is pushed, and
                    // it does not apply inside foreign content, where these
                    // names are ordinary elements whose children stay markup.
                    // html5ever 0.39 does not switch the tokenizer for script
                    // inside foreign content either (its foreign_start_tag has
                    // no ScriptData transition), so script is gated too.
                    // The switch is unconditional in HTML content because
                    // html5ever inserts title/textarea in every mode that
                    // accepts them (InHead, InBody, InTemplate, and the table
                    // modes that reprocess start tags through InBody); the
                    // modes that ignore them (the frameset family) also ignore
                    // every subsequent start tag, so no deep DOM can hide
                    // behind the raw-text state.
                    let foreign = in_foreign_content(&self.open_elements.borrow());
                    if is_void_html_element(name) && !foreign {
                        // Void HTML elements never push, but their start tags
                        // still run the implied-end-tag rules (hr closes p).
                        self.close_implied(name);
                    } else if !honor_self_closing {
                        // Inside foreign content, void HTML names are ordinary
                        // foreign elements that html5ever pushes.
                        self.push_element(&tag.name);
                    }
                    match name {
                        "title" | "textarea" if !foreign => TokenSinkResult::RawData(Rcdata),
                        "style" | "xmp" | "iframe" | "noembed" | "noframes" if !foreign => {
                            TokenSinkResult::RawData(Rawtext)
                        }
                        "script" if !foreign => TokenSinkResult::RawData(ScriptData),
                        "plaintext" if !foreign => TokenSinkResult::Plaintext,
                        _ => TokenSinkResult::Continue,
                    }
                }
                EndTag => {
                    self.close_element(&tag.name);
                    TokenSinkResult::Continue
                }
            },
            Token::CharacterTokens(text) => {
                if !text.is_empty() {
                    self.bump_node();
                }
                TokenSinkResult::Continue
            }
            Token::CommentToken(_) | Token::DoctypeToken(_) | Token::NullCharacterToken => {
                self.bump_node();
                TokenSinkResult::Continue
            }
            Token::EOFToken | Token::ParseError(_) => TokenSinkResult::Continue,
        }
    }
}

/// html5ever's default scope markers (tag_sets.rs: html_default_scope plus
/// the MathML text integration points and the SVG HTML integration points):
/// the elements that stop scope searches for end tags resolved through the
/// default scope. Per-tag scopes add their own markers (see
/// is_end_tag_scope_marker). Bare svg/math roots are NOT markers: the parser
/// pops through them to an in-scope target. Names are matched without
/// namespaces (the modeled stack carries none); the only collision, the HTML
/// <title>, stops searches that the <html> marker below it would stop anyway.
const GENERIC_SCOPE_MARKERS: &[&str] = &[
    "applet",
    "caption",
    "html",
    "table",
    "td",
    "th",
    "marquee",
    "object",
    "select",
    "template",
    // MathML text integration points and SVG HTML integration points.
    "mi",
    "mo",
    "mn",
    "ms",
    "mtext",
    "foreignobject",
    "desc",
    "title",
];

/// Scope markers that stop the search for an end tag's target element,
/// mirroring html5ever's end-tag rules (tag_sets.rs scopes and the
/// special_tag walk in process_end_tag_in_body).
fn is_end_tag_scope_marker(end_tag: &str, candidate: &str) -> bool {
    match end_tag {
        "table" | "thead" | "tbody" | "tfoot" | "tr" | "td" | "th" | "caption" | "colgroup" => {
            matches!(candidate, "html" | "table" | "template")
        }
        "li" => GENERIC_SCOPE_MARKERS.contains(&candidate) || matches!(candidate, "ol" | "ul"),
        "p" | "dd" | "dt" => GENERIC_SCOPE_MARKERS.contains(&candidate) || candidate == "button",
        "rt" | "rp" | "rb" | "rtc" => {
            GENERIC_SCOPE_MARKERS.contains(&candidate) || matches!(candidate, "ruby" | "rtc")
        }
        // The div family (plus button, form, the headings, and template)
        // pops to its match when the match is in generic scope.
        _ if is_generic_scope_end_tag(end_tag) => GENERIC_SCOPE_MARKERS.contains(&candidate),
        // Any other end tag: html5ever's walk ignores the token the moment
        // it reaches any special element whose name does not match.
        // Stopping only at the generic scope markers would let a deeper
        // match pop real nesting (for example </em> below <em><div><span>)
        // and undercount depth.
        _ => is_special_element(candidate),
    }
}

/// End tags whose html5ever rule is "pop to the matching element when it is
/// in generic scope, otherwise ignore": the div family plus button, form,
/// the headings, and template.
fn is_generic_scope_end_tag(end_tag: &str) -> bool {
    matches!(
        end_tag,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "button"
            | "center"
            | "details"
            | "dialog"
            | "dir"
            | "div"
            | "dl"
            | "fieldset"
            | "figcaption"
            | "figure"
            | "footer"
            | "form"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "header"
            | "hgroup"
            | "listing"
            | "main"
            | "menu"
            | "nav"
            | "ol"
            | "pre"
            | "section"
            | "summary"
            | "template"
            | "ul"
    )
}

/// html5ever's special element category (tag_sets.rs special_tag): the names
/// that stop the any-other-end-tag walk (process_end_tag_in_body) and, minus
/// address/div/p, the list-item implied-close walk. Integration points are
/// NOT special there, and bare svg/math never were.
fn is_special_element(candidate: &str) -> bool {
    matches!(
        candidate,
        "address"
            | "applet"
            | "area"
            | "article"
            | "aside"
            | "base"
            | "basefont"
            | "bgsound"
            | "blockquote"
            | "body"
            | "br"
            | "button"
            | "caption"
            | "center"
            | "col"
            | "colgroup"
            | "dd"
            | "details"
            | "dir"
            | "div"
            | "dl"
            | "dt"
            | "embed"
            | "fieldset"
            | "figcaption"
            | "figure"
            | "footer"
            | "form"
            | "frame"
            | "frameset"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "head"
            | "header"
            | "hgroup"
            | "hr"
            | "html"
            | "iframe"
            | "img"
            | "input"
            | "isindex"
            | "li"
            | "link"
            | "listing"
            | "main"
            | "marquee"
            | "menu"
            | "meta"
            | "nav"
            | "noembed"
            | "noframes"
            | "noscript"
            | "object"
            | "ol"
            | "p"
            | "param"
            | "plaintext"
            | "pre"
            | "script"
            | "section"
            | "select"
            | "source"
            | "style"
            | "summary"
            | "table"
            | "tbody"
            | "td"
            | "template"
            | "textarea"
            | "tfoot"
            | "th"
            | "thead"
            | "title"
            | "tr"
            | "track"
            | "ul"
            | "wbr"
            | "xmp"
    )
}

/// Apply html5ever's implied-end-tag rules for a start tag to the modeled
/// stack. Returns true when html5ever would ignore the token outright
/// (table-family start tags outside a table in body context), in which case
/// the caller must not push the element either.
///
/// Every rule closes only what html5ever would really close: a search that
/// reaches past a scope boundary would truncate real nesting and undercount
/// depth, so each walk stops at the markers of its own rule.
fn close_implied_before_start(open: &mut Vec<LocalName>, name: &str) -> bool {
    if name == "a"
        && let Some(position) = open.iter().rposition(|candidate| candidate.as_ref() == "a")
    {
        open.remove(position);
    }

    // HTML5 implicitly closes an open <p> when a block-level start tag
    // arrives: html5ever closes the p whenever it is in button scope,
    // popping its inline descendants with it (close_p_element_in_button_
    // scope). Modeling only the innermost-p case left stale paragraphs
    // below intervening elements; a later p start would then find the
    // stale p and truncate through those elements, UNDERcounting the
    // nesting the parser really keeps open. Void tags reach this hook
    // through close_implied (so <hr> does close an open <p>) without being
    // pushed themselves.
    if is_paragraph_closing_element(name) {
        for (position, candidate) in open.iter().enumerate().rev() {
            let candidate = candidate.as_ref();
            if candidate == "p" {
                open.truncate(position);
                break;
            }
            if candidate == "button" || GENERIC_SCOPE_MARKERS.contains(&candidate) {
                break;
            }
        }
    }

    // html5ever pops the current node only when it is already a heading
    // (a heading start tag does not pop intervening elements), so the
    // preflight mirrors exactly that: pop the innermost element when it is
    // a heading.
    if is_heading_element(name)
        && open.last().is_some_and(|candidate| is_heading_element(candidate.as_ref()))
    {
        open.pop();
    }

    // option/optgroup: html5ever only pops the innermost open element here.
    // Inside a select, an optgroup start additionally closes an optgroup
    // that is the current node; in plain body context it does not, and a
    // deeper search would truncate real nesting opened between the pairs.
    if matches!(name, "option" | "optgroup") {
        if open.last().is_some_and(|candidate| candidate.as_ref() == "option") {
            open.pop();
        }
        if name == "optgroup"
            && open.iter().any(|candidate| candidate.as_ref() == "select")
            && open.last().is_some_and(|candidate| candidate.as_ref() == "optgroup")
        {
            open.pop();
        }
        return false;
    }

    // rt/rp: html5ever pops the current node only when it is an rt/rp.
    if matches!(name, "rt" | "rp") {
        if open.last().is_some_and(|candidate| matches!(candidate.as_ref(), "rt" | "rp")) {
            open.pop();
        }
        return false;
    }

    // Table-family start tags outside a table are ignored outright by
    // html5ever in body context (the caption/col/.../tr arm of InBody).
    // Pushing them would let a later implied close truncate the real
    // nesting opened between stray cells, undercounting depth; ignoring
    // matches the parser exactly.
    if matches!(
        name,
        "caption" | "col" | "colgroup" | "tbody" | "td" | "tfoot" | "th" | "thead" | "tr"
    ) {
        if !open.iter().any(|candidate| candidate.as_ref() == "table") {
            return true;
        }
        // Inside a table, every table-family start first clears the stack
        // back to the table context (html5ever InTable:
        // pop_until_current(table_scope) for caption, col, colgroup,
        // tbody/tfoot/thead, and td/th/tr alike): foster-parented stray
        // content above the table leaves the stack and previous rows or
        // cells close. Without the clear, that stray content accumulates
        // as phantom depth and valid repaired table markup can be falsely
        // rejected at max_xml_depth. The phantom tbody/tr wrappers
        // html5ever inserts afterwards are not modeled (a bounded,
        // documented deviation), and a template above the table stops the
        // clear exactly as table_scope does.
        if let Some(clear_to) = open
            .iter()
            .rposition(|candidate| matches!(candidate.as_ref(), "table" | "template" | "html"))
            .map(|index| index + 1)
        {
            open.truncate(clear_to);
        }
        return false;
    }

    // li / dd / dt: html5ever walks from the innermost element and closes
    // the matching target only when no special element other than
    // address/div/p intervenes. An unscoped search would close a target
    // below, for example, an intervening table or list and undercount the
    // nesting the parser keeps open.
    if matches!(name, "li" | "dd" | "dt") {
        let targets: &[&str] = if name == "li" { &["li"] } else { &["dd", "dt"] };
        for (position, candidate) in open.iter().enumerate().rev() {
            let candidate = candidate.as_ref();
            if targets.contains(&candidate) {
                open.truncate(position);
                break;
            }
            if is_special_element(candidate) && !matches!(candidate, "address" | "div" | "p") {
                break;
            }
        }
        return false;
    }

    false
}

fn is_heading_element(name: &str) -> bool {
    matches!(name, "h1" | "h2" | "h3" | "h4" | "h5" | "h6")
}

/// Block-level start tags that make HTML5 implicitly close an open <p>
/// (html5ever's in-body close_p_element_in_button_scope arms; table closes
/// <p> only outside quirks mode, and the preflight assumes standards mode).
fn is_paragraph_closing_element(name: &str) -> bool {
    matches!(
        name,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "center"
            | "details"
            | "dialog"
            | "dir"
            | "div"
            | "dl"
            | "fieldset"
            | "figcaption"
            | "figure"
            | "footer"
            | "form"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "header"
            | "hgroup"
            | "hr"
            | "listing"
            | "main"
            | "menu"
            | "nav"
            | "ol"
            | "p"
            | "plaintext"
            | "pre"
            | "search"
            | "section"
            | "summary"
            | "table"
            | "ul"
    )
}

/// True when the innermost relevant ancestor puts the parser in foreign
/// content: an svg/math root with no HTML integration point above it.
fn in_foreign_content(open: &[LocalName]) -> bool {
    for candidate in open.iter().rev() {
        match candidate.as_ref() {
            "foreignobject" | "desc" | "title" | "mi" | "mo" | "mn" | "ms" | "mtext"
            | "annotation-xml" => return false,
            "svg" | "math" => return true,
            _ => {}
        }
    }
    false
}

/// Index of the outermost svg/math root reachable from the innermost open
/// element without crossing an integration point, when the parser is inside
/// foreign content. Elements at or above that index form the foreign region
/// that html5ever's foreign-phase end-tag walk traverses (matching names in
/// any namespace) before reprocessing the token in the HTML insertion mode.
fn foreign_root_index(open: &[LocalName]) -> Option<usize> {
    let mut outermost = None;
    for (index, candidate) in open.iter().enumerate().rev() {
        match candidate.as_ref() {
            "foreignobject" | "desc" | "title" | "mi" | "mo" | "mn" | "ms" | "mtext"
            | "annotation-xml" => break,
            "svg" | "math" => outermost = Some(index),
            _ => {}
        }
    }
    outermost
}

fn html5_self_closing_is_honored(open: &[LocalName], name: &str) -> bool {
    if matches!(name, "svg" | "math") {
        return true;
    }

    in_foreign_content(open) && !is_foreign_content_html_breakout(name)
}

fn is_foreign_content_html_breakout(name: &str) -> bool {
    matches!(
        name,
        "b" | "big"
            | "blockquote"
            | "body"
            | "br"
            | "center"
            | "code"
            | "dd"
            | "div"
            | "dl"
            | "dt"
            | "em"
            | "embed"
            | "font"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "head"
            | "hr"
            | "i"
            | "img"
            | "li"
            | "listing"
            | "menu"
            | "meta"
            | "nobr"
            | "ol"
            | "p"
            | "pre"
            | "ruby"
            | "s"
            | "small"
            | "span"
            | "strike"
            | "strong"
            | "sub"
            | "sup"
            | "table"
            | "tt"
            | "u"
            | "ul"
            | "var"
    )
}

/// Elements that never stay on the open-element stack in HTML content: the
/// void elements, plus frame. html5ever ignores a stray frame in body
/// context (rules.rs, the caption/col/.../tr ignore arm) and inserts then
/// immediately pops it in frameset context (insert_and_pop_element_for), so
/// stacking frames would falsely report max_xml_depth for framesets with
/// more than MAX_XML_DEPTH sibling frames. In foreign content a frame is an
/// ordinary foreign element that does push (callers gate on that).
fn is_void_html_element(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "frame"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

fn preflight_html_complexity(text: &str) -> Result<(), ConvertError> {
    const CHUNK_BYTES: usize = 64 * 1024;
    let tokenizer = Tokenizer::new(HtmlComplexitySink::default(), Default::default());
    let input = BufferQueue::default();
    let mut offset = 0usize;

    while offset < text.len() {
        let mut end = offset.saturating_add(CHUNK_BYTES).min(text.len());
        while end > offset && !text.is_char_boundary(end) {
            end -= 1;
        }
        input.push_back(StrTendril::from(&text[offset..end]));
        let _ = tokenizer.feed(&input);
        check_preflight_limits(&tokenizer.sink)?;
        offset = end;
    }

    tokenizer.end();
    check_preflight_limits(&tokenizer.sink)
}

fn check_preflight_limits(sink: &HtmlComplexitySink) -> Result<(), ConvertError> {
    if sink.node_limit_exceeded.get() {
        return Err(ConvertError::ResourceLimit {
            limit: "max_xml_nodes",
            detail: format!("HTML token stream has more than {} nodes", limits::MAX_XML_NODES),
        });
    }
    if sink.depth_limit_exceeded.get() {
        return Err(ConvertError::ResourceLimit {
            limit: "max_xml_depth",
            detail: format!(
                "HTML source nesting depth exceeds {} before DOM construction",
                limits::MAX_XML_DEPTH
            ),
        });
    }
    Ok(())
}

fn adapt_element(
    source: ElementRef<'_>,
    depth: usize,
    node_count: &mut usize,
) -> Result<Element, ConvertError> {
    if depth > limits::MAX_XML_DEPTH {
        return Err(ConvertError::ResourceLimit {
            limit: "max_xml_depth",
            detail: format!("HTML element nesting depth {depth} exceeds {}", limits::MAX_XML_DEPTH),
        });
    }
    bump_node_count(node_count)?;

    let value = source.value();
    let attrs = value
        .attrs
        .iter()
        .map(|(name, value)| Attr {
            ns: optional_namespace(name.ns.as_ref()),
            local: name.local.as_ref().to_owned(),
            value: value.to_string(),
        })
        .collect();

    let mut children = Vec::new();
    for child in source.children() {
        match child.value() {
            HtmlNode::Element(_) => {
                if let Some(element) = ElementRef::wrap(child) {
                    children.push(Node::Elem(adapt_element(element, depth + 1, node_count)?));
                }
            }
            HtmlNode::Text(text) => {
                bump_node_count(node_count)?;
                children.push(Node::Text(text.text.to_string()));
            }
            HtmlNode::Comment(_) | HtmlNode::Doctype(_) | HtmlNode::ProcessingInstruction(_) => {
                bump_node_count(node_count)?;
            }
            HtmlNode::Document | HtmlNode::Fragment => {}
        }
    }

    Ok(Element {
        ns: optional_namespace(value.name.ns.as_ref()),
        local: value.name.local.as_ref().to_owned(),
        attrs,
        children,
    })
}

fn optional_namespace(namespace: &str) -> Option<Rc<str>> {
    (!namespace.is_empty()).then(|| Rc::<str>::from(namespace))
}

fn bump_node_count(node_count: &mut usize) -> Result<(), ConvertError> {
    *node_count = node_count.saturating_add(1);
    if *node_count > limits::MAX_XML_NODES {
        return Err(ConvertError::ResourceLimit {
            limit: "max_xml_nodes",
            detail: format!("HTML tree has more than {} nodes", limits::MAX_XML_NODES),
        });
    }
    Ok(())
}

struct StandaloneCtx;

impl HtmlCtx for StandaloneCtx {
    fn link_target(&self, href: &str) -> Option<LinkTarget> {
        let href = href.trim();
        if href.is_empty() {
            return None;
        }
        if let Some(fragment) = href.strip_prefix('#') {
            if fragment.is_empty() {
                // A bare "#" points at the document itself. Keep it as a
                // relative URL so the link survives rendering instead of
                // becoming an unresolvable empty anchor.
                return Some(LinkTarget::Relative(href.to_owned()));
            }
            let fragment = crate::package::path::decode_fragment(fragment);
            return Some(LinkTarget::Anchor(fragment));
        }
        Some(if is_absolute_uri(href) {
            LinkTarget::External(href.to_owned())
        } else {
            LinkTarget::Relative(href.to_owned())
        })
    }

    fn image_source(&self, src: &str) -> Result<Option<ImageSource>, ConvertError> {
        let src = src.trim();
        if src.is_empty() {
            return Ok(None);
        }
        Ok(Some(ImageSource::External(src.to_owned())))
    }

    fn anchor_id(&self, raw: &str) -> AnchorId {
        raw.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charset_sniff_accepts_meta_and_http_equiv_forms() {
        assert_eq!(
            sniff_meta_charset(b"<meta charset='windows-1252'>").map(Encoding::name),
            Some("windows-1252")
        );
        assert_eq!(
            sniff_meta_charset(
                b"<meta http-equiv=content-type content='text/html; charset=iso-8859-1'>"
            )
            .map(Encoding::name),
            Some("windows-1252")
        );
    }

    #[test]
    fn charset_sniff_ignores_non_meta_text_and_attributes() {
        assert_eq!(sniff_meta_charset(b"<p data-note='charset=windows-1252'>utf-8</p>"), None);
        assert_eq!(sniff_meta_charset(b"<!-- <meta charset=windows-1252> --><p>utf-8</p>"), None);
        assert_eq!(
            sniff_meta_charset(b"<script>const x='<meta charset=windows-1252>';</script>"),
            None
        );
    }
}
