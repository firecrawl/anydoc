//! RTF frontend: a position-explicit lexer feeding a state machine, with the
//! font/style/list tables parsed into typed definitions up front. Numbering
//! comes from the list tables - never guessed from label text. Page
//! headers/footers are excluded (fixed policy).

mod lexer;
mod table;
mod tables;

use crate::error::ConvertError;
use crate::model::{Block, Document, Inline, Note, NoteKind, Style, VertAlign, inlines_are_empty};
use crate::shared::blockstyle::{BlockStyle, StyledRun};
use crate::shared::delta::rebase_emphasis;
use crate::shared::fields::field_result;
use crate::shared::list::{ListEntry, ListKey, MarkerKind, flush_list};
use crate::shared::text::clean_text;
use lexer::{Lexer, Token};
use std::collections::HashMap;
use table::TableState;
use tables::{LIST_LEVELS, Prelude, codepage_encoding, parse_prelude};

pub fn parse(bytes: &[u8]) -> Result<Document, ConvertError> {
    if !bytes.starts_with(b"{\\rtf") {
        return Err(ConvertError::malformed("not an RTF file"));
    }
    // The code page must be known before the font table decodes; scan the
    // header for \ansicpg first.
    let default_encoding = scan_codepage(bytes);
    let prelude = parse_prelude(bytes, default_encoding);
    let mut parser = Parser::new(bytes, prelude, default_encoding);
    parser.run()?;
    parser.finish()
}

fn scan_codepage(bytes: &[u8]) -> &'static encoding_rs::Encoding {
    // \ansicpg sits in the header, but generator comments and extra header
    // words can push it past any fixed prefix; the lexer scan is linear and
    // stops at the first match.
    let mut lexer = Lexer::new(bytes);
    while let Some(token) = lexer.next_token() {
        if let Token::Word { name: "ansicpg", param: Some(cp) } = token {
            return codepage_encoding(cp.max(0) as u32);
        }
    }
    encoding_rs::WINDOWS_1252
}

#[derive(Clone, Copy, PartialEq)]
enum Capture {
    None,
    ListText,
    FieldInstr,
    Bookmark,
    /// Inside a `\pict` destination: bytes route to the picture collector.
    Pict,
}

#[derive(Clone, Copy)]
struct CharState {
    style: Style,
    font: Option<i32>,
    uc_skip: u32,
    in_table: bool,
    itap: usize,
    ilvl: usize,
    ls: Option<i32>,
    legacy_list: Option<MarkerKind>,
    outline: Option<u8>,
    /// The block container this paragraph's style names.
    block: Option<BlockStyle>,
    /// Emphasis the paragraph style itself carries, subtracted from headings.
    style_base: Style,
    suppress: bool,
    capture: Capture,
    note: Option<NoteKind>,
}

impl Default for CharState {
    fn default() -> Self {
        CharState {
            style: Style::PLAIN,
            font: None,
            uc_skip: 1,
            in_table: false,
            itap: 1,
            ilvl: 0,
            ls: None,
            legacy_list: None,
            outline: None,
            block: None,
            style_base: Style::PLAIN,
            suppress: false,
            capture: Capture::None,
            note: None,
        }
    }
}

struct FieldFrame {
    depth: usize,
    instr: String,
    start: usize,
}

struct NoteFrame {
    depth: usize,
    start: usize,
    kind: NoteKind,
}

/// Per-instance numbering counters with restart-on-shallower semantics.
#[derive(Default)]
struct Counters {
    state: HashMap<i32, ([u64; LIST_LEVELS], [bool; LIST_LEVELS])>,
}

impl Counters {
    fn next(&mut self, ls: i32, level: usize, start: u64) -> u64 {
        let level = level.min(LIST_LEVELS - 1);
        let (values, started) = self.state.entry(ls).or_default();
        let value = if started[level] { values[level].saturating_add(1) } else { start };
        values[level] = value;
        started[level] = true;
        for s in started.iter_mut().skip(level + 1) {
            *s = false;
        }
        value
    }

    /// Advance a table-defined list level and render its composite label
    /// against the live counter values.
    fn next_labeled(
        &mut self,
        ls: i32,
        level: usize,
        levels: &[tables::ListLevelDef; LIST_LEVELS],
    ) -> (u64, Option<String>) {
        let level = level.min(LIST_LEVELS - 1);
        let def = &levels[level];
        let value = self.next(ls, level, def.start);
        let (values, started) = self.state.entry(ls).or_default();
        let label = def.marker.and_then(|marker| {
            crate::shared::numbering::composite_label(
                &def.pattern,
                marker,
                value,
                |l| levels[l.min(LIST_LEVELS - 1)].marker.unwrap_or(MarkerKind::Decimal),
                |l| {
                    let l = l.min(LIST_LEVELS - 1);
                    if started[l] { values[l] } else { levels[l].start }
                },
            )
        });
        (value, label)
    }

    /// Pin a counter to a number taken from the source (legacy `\pn` labels).
    fn seed(&mut self, ls: i32, level: usize, value: u64) {
        let level = level.min(LIST_LEVELS - 1);
        let (values, started) = self.state.entry(ls).or_default();
        values[level] = value;
        started[level] = true;
        for s in started.iter_mut().skip(level + 1) {
            *s = false;
        }
    }
}

/// Byte-level text decoding: pending code-page bytes, `\uN` fallback skips,
/// and surrogate pairing.
struct TextDecoder {
    default_encoding: &'static encoding_rs::Encoding,
    pending: Vec<u8>,
    skip: u32,
    surrogate: Option<u16>,
}

impl TextDecoder {
    fn new(default_encoding: &'static encoding_rs::Encoding) -> Self {
        TextDecoder { default_encoding, pending: Vec::new(), skip: 0, surrogate: None }
    }

    /// Buffer one text byte, honoring an active `\uN` fallback skip.
    fn byte(&mut self, b: u8) {
        if self.skip > 0 {
            self.skip -= 1;
        } else {
            self.pending.push(b);
        }
    }

    /// True when an active fallback skip consumed the character.
    fn skip_char(&mut self) -> bool {
        if self.skip > 0 {
            self.skip -= 1;
            true
        } else {
            false
        }
    }

    /// Decode and take the buffered bytes with the current font's encoding.
    fn take_pending(&mut self, encoding: Option<&'static encoding_rs::Encoding>) -> Option<String> {
        if self.pending.is_empty() {
            return None;
        }
        let bytes = std::mem::take(&mut self.pending);
        let (text, _, _) = encoding.unwrap_or(self.default_encoding).decode(&bytes);
        Some(text.into_owned())
    }

    /// `\uN`: the completed scalar (surrogate pairs are held and combined),
    /// arming the `uc_skip`-length fallback skip.
    fn unicode(&mut self, param: Option<i32>, uc_skip: u32) -> Option<char> {
        // A new \u ends the previous one's fallback range.
        self.skip = 0;
        let n = param?;
        let code = if n < 0 { (n + 65536) as u32 } else { n as u32 };
        let out = match (self.surrogate.take(), code as u16) {
            // High surrogate: hold for its pair.
            (None, unit @ 0xD800..=0xDBFF) => {
                self.surrogate = Some(unit);
                None
            }
            (Some(high), low @ 0xDC00..=0xDFFF) => {
                let combined = 0x10000 + ((high as u32 - 0xD800) << 10) + (low as u32 - 0xDC00);
                char::from_u32(combined)
            }
            (_, _) => char::from_u32(code),
        };
        self.skip = uc_skip;
        out
    }
}

/// An accumulating `\pict` destination: the payload arrives as ASCII hex
/// characters (or one `\binN` run), typed by a format control word.
#[derive(Default)]
struct PictState {
    /// Group depth of the `\pict` destination itself: payload bytes live at
    /// this depth only (subgroups like `\*\picprop` carry properties).
    depth: usize,
    hex: Vec<u8>,
    binary: Option<Vec<u8>>,
    /// (media type, extension) from `\pngblip`/`\jpegblip`/`\emfblip`/
    /// `\wmetafile`; `None` = unsupported format.
    format: Option<(&'static str, &'static str)>,
}

impl PictState {
    /// The decoded payload bytes.
    fn payload(self) -> Vec<u8> {
        if let Some(binary) = self.binary {
            return binary;
        }
        let mut out = Vec::with_capacity(self.hex.len() / 2);
        let mut high: Option<u8> = None;
        for &b in &self.hex {
            let Some(digit) = (b as char).to_digit(16) else {
                continue;
            };
            match high.take() {
                Some(h) => out.push((h << 4) | digit as u8),
                None => high = Some(digit as u8),
            }
        }
        out
    }
}

/// Field, note, bookmark, and list-label destination state: frames open at a
/// group depth and close when the group stack unwinds past it.
#[derive(Default)]
struct Destinations {
    fields: Vec<FieldFrame>,
    note_frames: Vec<NoteFrame>,
    notes: Vec<Note>,
    bookmark: String,
    /// Captured `\listtext` label for the current paragraph.
    listtext: Option<String>,
    /// The `\pict` destination currently accumulating, if any.
    pict: Option<PictState>,
}

impl Destinations {
    /// Close field frames opened deeper than `depth`, folding their results
    /// back into the inline stream.
    fn close_fields(&mut self, depth: usize, inlines: &mut Vec<Inline>) {
        while self.fields.last().is_some_and(|f| f.depth > depth) {
            let frame = self.fields.pop().unwrap();
            let start = frame.start.min(inlines.len());
            let content: Vec<Inline> = inlines.drain(start..).collect();
            inlines.extend(field_result(&frame.instr, content));
        }
    }

    /// Close note frames opened deeper than `depth`, replacing their content
    /// with a reference to the collected note.
    fn close_notes(&mut self, depth: usize, inlines: &mut Vec<Inline>) {
        while self.note_frames.last().is_some_and(|f| f.depth > depth) {
            let frame = self.note_frames.pop().unwrap();
            let start = frame.start.min(inlines.len());
            let content: Vec<Inline> = inlines.drain(start..).collect();
            if !inlines_are_empty(&content) {
                let id = format!("rtf{}", self.notes.len());
                self.notes.push(Note {
                    id: id.clone(),
                    kind: frame.kind,
                    blocks: vec![Block::Paragraph(content)],
                });
                inlines.push(Inline::NoteRef(id));
            }
        }
    }

    /// Emit a completed bookmark capture as an anchor.
    fn close_bookmark(&mut self, still_capturing: bool, inlines: &mut Vec<Inline>) {
        if !still_capturing && !self.bookmark.is_empty() {
            let name = std::mem::take(&mut self.bookmark);
            let name = name.trim().to_string();
            if !name.is_empty() {
                inlines.push(Inline::Anchor(name));
            }
        }
    }
}

/// Destinations whose content is excluded: headers/footers by fixed policy,
/// metadata and binary destinations because they carry no document text.
const SUPPRESSED_DESTINATIONS: &[&str] = &[
    "fonttbl",
    "colortbl",
    "stylesheet",
    "info",
    "object",
    "header",
    "footer",
    "headerl",
    "headerr",
    "headerf",
    "footerl",
    "footerr",
    "footerf",
    "ftnsep",
    "ftnsepc",
    "aftnsep",
    "aftnsepc",
    "xmlnstbl",
    "themedata",
    "colorschememapping",
    "datastore",
    "latentstyles",
    "listtable",
    "listoverridetable",
    "rsidtbl",
    "generator",
    "filetbl",
    "revtbl",
    "datafield",
    "bkmkend",
    "annotation",
    "atnid",
    "atnauthor",
    "template",
    "defchp",
    "defpap",
    "panose",
    "falt",
    "objdata",
    "blipuid",
    "nonshppict",
    "wgrffmtfilter",
    "pgdsctbl",
    "docvar",
    "sp",
    "sn",
    "sv",
    "shpinst",
    "background",
    "userprops",
    "operator",
    "author",
    "title",
    "subject",
    "keywords",
    "doccomm",
    "creatim",
    "revtim",
    "printim",
];

struct Parser<'a> {
    lexer: Lexer<'a>,
    stack: Vec<CharState>,
    state: CharState,
    prelude: Prelude,
    decoder: TextDecoder,
    recovered: bool,

    inlines: Vec<Inline>,
    blocks: Vec<Block>,
    list_run: Vec<ListEntry>,
    styled: StyledRun,
    counters: Counters,
    table: TableState,
    dest: Destinations,
    assets: crate::shared::assets::AssetSink,
}

impl<'a> Parser<'a> {
    fn new(
        bytes: &'a [u8],
        prelude: Prelude,
        default_encoding: &'static encoding_rs::Encoding,
    ) -> Self {
        Parser {
            lexer: Lexer::new(bytes),
            stack: Vec::new(),
            state: CharState::default(),
            prelude,
            decoder: TextDecoder::new(default_encoding),
            recovered: false,
            inlines: Vec::new(),
            blocks: Vec::new(),
            list_run: Vec::new(),
            styled: StyledRun::default(),
            counters: Counters::default(),
            table: TableState::new(),
            dest: Destinations::default(),
            assets: crate::shared::assets::AssetSink::new(),
        }
    }

    fn run(&mut self) -> Result<(), ConvertError> {
        while let Some(token) = self.lexer.next_token() {
            match token {
                Token::Open => {
                    self.flush_pending();
                    self.stack.push(self.state);
                }
                Token::Close => {
                    self.flush_pending();
                    match self.stack.pop() {
                        Some(prev) => self.state = prev,
                        None => self.recovered = true,
                    }
                    let depth = self.stack.len();
                    self.dest.close_fields(depth, &mut self.inlines);
                    self.dest.close_notes(depth, &mut self.inlines);
                    self.dest
                        .close_bookmark(self.state.capture == Capture::Bookmark, &mut self.inlines);
                    // The pict destination finishes when the stack unwinds
                    // past its own group (inner property groups close first).
                    if self.dest.pict.as_ref().is_some_and(|p| self.stack.len() < p.depth) {
                        self.finish_pict()?;
                    }
                }
                Token::Word { name, param } => self.control_word(name, param)?,
                Token::Symbol(b) => self.control_symbol(b),
                Token::Hex(b) | Token::Byte(b) => {
                    if self.state.capture == Capture::Pict {
                        if let Some(pict) = &mut self.dest.pict
                            && self.stack.len() == pict.depth
                        {
                            pict.hex.push(b);
                        }
                    } else if self.accepts_text() {
                        self.decoder.byte(b);
                    }
                }
                Token::Bin(payload) => {
                    if self.state.capture == Capture::Pict
                        && let Some(pict) = &mut self.dest.pict
                        && self.stack.len() == pict.depth
                    {
                        pict.binary = Some(payload.to_vec());
                    } else {
                        log::debug!("skipping {} bytes of embedded binary data", payload.len());
                    }
                }
            }
        }
        if !self.stack.is_empty() {
            self.recovered = true;
        }
        if self.recovered {
            log::warn!("recovered unbalanced rtf groups");
        }
        self.flush_pending();
        self.end_paragraph()
    }

    fn accepts_text(&self) -> bool {
        self.state.capture != Capture::None || !self.state.suppress
    }

    fn control_symbol(&mut self, b: u8) {
        match b {
            b'~' => self.push_char('\u{a0}'),
            b'-' => {}
            b'_' => self.push_char('-'),
            b'*' => self.state.suppress = true,
            b'\\' | b'{' | b'}' => self.push_char(b as char),
            _ => {}
        }
    }

    /// Dispatch a control word to its subsystem handler; unhandled words
    /// that name a suppressed destination silence their group.
    fn control_word(&mut self, word: &str, param: Option<i32>) -> Result<(), ConvertError> {
        if self.text_control(word, param)? {
            return Ok(());
        }
        if self.table_control(word, param)? {
            return Ok(());
        }
        if self.list_control(word, param) {
            return Ok(());
        }
        if self.object_control(word) {
            return Ok(());
        }
        if SUPPRESSED_DESTINATIONS.contains(&word) {
            self.flush_pending();
            self.state.suppress = true;
            self.state.capture = Capture::None;
        }
        Ok(())
    }

    /// Character- and paragraph-level controls: text encoding, styling,
    /// paragraph and line breaks, and special characters.
    fn text_control(&mut self, word: &str, param: Option<i32>) -> Result<bool, ConvertError> {
        let on = param != Some(0);
        match word {
            "u" => {
                if self.accepts_text() {
                    self.flush_pending();
                    if let Some(c) = self.decoder.unicode(param, self.state.uc_skip) {
                        self.push_text(c.to_string());
                    }
                }
            }
            "uc" => self.state.uc_skip = param.unwrap_or(1).max(0) as u32,
            "f" => {
                self.flush_pending();
                self.state.font = param;
            }
            "b" => self.set_style(|s| s.bold = on),
            "i" => self.set_style(|s| s.italic = on),
            "strike" | "striked" => self.set_style(|s| s.strike = on),
            "super" => {
                self.set_vert_align(if on { VertAlign::Superscript } else { VertAlign::Baseline })
            }
            "sub" => {
                self.set_vert_align(if on { VertAlign::Subscript } else { VertAlign::Baseline })
            }
            "nosupersub" => self.set_vert_align(VertAlign::Baseline),
            // \upN and \dnN carry an offset in half-points rather than a
            // toggle, and the spec's default of 6 applies when it is absent.
            "up" | "dn" => {
                let raised = word == "up";
                self.set_vert_align(match param.unwrap_or(6) {
                    0 => VertAlign::Baseline,
                    _ if raised => VertAlign::Superscript,
                    _ => VertAlign::Subscript,
                })
            }
            "plain" => {
                self.flush_pending();
                let font = self.state.font;
                self.state.style = Style::PLAIN;
                self.state.font = font;
            }
            "s" => {
                // Paragraph style: outline level for headings plus its
                // formatting delta as the new base.
                let def = param.and_then(|id| self.prelude.styles.get(&id).copied());
                if let Some(def) = def {
                    self.flush_pending();
                    self.state.outline = def.outline;
                    self.state.block = def.block;
                    self.state.style = def.delta.apply(self.state.style);
                    self.state.style_base = self.state.style;
                }
            }
            "par" | "sect" => {
                self.flush_pending();
                if self.state.note.is_some() {
                    self.inlines.push(Inline::LineBreak);
                } else if !self.state.suppress {
                    self.end_paragraph()?;
                }
            }
            "pard" => {
                self.flush_pending();
                self.state.in_table = false;
                self.state.itap = 1;
                self.state.ilvl = 0;
                self.state.ls = None;
                self.state.legacy_list = None;
                self.state.outline = None;
                self.state.block = None;
                self.state.style_base = Style::PLAIN;
            }
            // \page and \column break the flow without ending the
            // paragraph; the page they start is unrepresentable, the word
            // boundary they carry is not.
            "line" | "lbr" | "page" | "column" => {
                self.flush_pending();
                if !self.state.suppress {
                    self.inlines.push(Inline::LineBreak);
                }
            }
            "tab" => self.push_char(' '),
            "emdash" => self.push_char('\u{2014}'),
            "endash" => self.push_char('\u{2013}'),
            "lquote" => self.push_char('\u{2018}'),
            "rquote" => self.push_char('\u{2019}'),
            "ldblquote" => self.push_char('\u{201c}'),
            "rdblquote" => self.push_char('\u{201d}'),
            "bullet" => self.push_char('\u{2022}'),
            "enspace" | "emspace" | "qmspace" => self.push_char(' '),
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Table controls, delegated to the per-depth [`TableState`].
    fn table_control(&mut self, word: &str, param: Option<i32>) -> Result<bool, ConvertError> {
        match word {
            "intbl" => self.state.in_table = true,
            "itap" => {
                self.state.itap = param.unwrap_or(1).clamp(0, 8) as usize;
                if self.state.itap > 1 {
                    self.state.in_table = true;
                }
            }
            "trowd" => {
                if self.table_active() {
                    let depth = self.state.itap.max(1);
                    self.table.begin_row(depth);
                }
            }
            "trhdr" => {
                if self.table_active() {
                    let depth = self.state.itap.max(1);
                    self.table.mark_header_row(depth);
                }
            }
            "clmgf" => self.pending_cell_prop(|p| p.merge_first = true),
            "clmrg" => self.pending_cell_prop(|p| p.merge_cont = true),
            "clvmgf" => self.pending_cell_prop(|p| p.vmerge_first = true),
            "clvmrg" => self.pending_cell_prop(|p| p.vmerge_cont = true),
            "cellx" => {
                if self.table_active() {
                    let depth = self.state.itap.max(1);
                    self.table.declare_cell(depth, param.unwrap_or(0) as i64);
                }
            }
            "cell" => {
                self.flush_pending();
                if self.table_active() {
                    self.end_cell(1)?;
                }
            }
            "nestcell" => {
                self.flush_pending();
                if self.table_active() {
                    self.end_cell(self.state.itap.max(2))?;
                }
            }
            "row" => {
                self.flush_pending();
                if self.table_active() {
                    self.end_row(1)?;
                }
            }
            "nestrow" => {
                self.flush_pending();
                if self.table_active() {
                    self.end_row(self.state.itap.max(2))?;
                }
            }
            // Nested row properties arrive in a `{\*\nesttableprops ...}`
            // destination; its \trowd/\cellx/\nestrow must still act.
            "nesttableprops" => self.state.suppress = false,
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// List and outline controls.
    fn list_control(&mut self, word: &str, param: Option<i32>) -> bool {
        match word {
            "outlinelevel" => {
                if let Some(n) = param
                    && (0..9).contains(&n)
                {
                    self.state.outline = Some((n + 1) as u8);
                }
            }
            "ilvl" => self.state.ilvl = param.unwrap_or(0).clamp(0, 8) as usize,
            "ls" => self.state.ls = param,
            "listtext" | "pntext" => {
                self.flush_pending();
                self.state.capture = Capture::ListText;
                if self.dest.listtext.is_none() {
                    self.dest.listtext = Some(String::new());
                }
            }
            "pnlvlblt" => self.state.legacy_list = Some(MarkerKind::Bullet),
            "pnlvlbody" | "pndec" => self.state.legacy_list = Some(MarkerKind::Decimal),
            _ => return false,
        }
        true
    }

    /// Field, footnote, and bookmark controls.
    fn object_control(&mut self, word: &str) -> bool {
        match word {
            "field" => {
                self.flush_pending();
                if !self.state.suppress {
                    self.dest.fields.push(FieldFrame {
                        depth: self.stack.len(),
                        instr: String::new(),
                        start: self.inlines.len(),
                    });
                }
            }
            "fldinst" => {
                self.flush_pending();
                if !self.dest.fields.is_empty() {
                    self.state.capture = Capture::FieldInstr;
                }
                self.state.suppress = true;
            }
            "fldrslt" => {
                self.flush_pending();
                self.state.capture = Capture::None;
            }
            "footnote" => {
                self.flush_pending();
                self.state.note = Some(NoteKind::Footnote);
                self.state.suppress = false;
                self.state.capture = Capture::None;
                self.dest.note_frames.push(NoteFrame {
                    depth: self.stack.len(),
                    start: self.inlines.len(),
                    kind: NoteKind::Footnote,
                });
            }
            "ftnalt" => {
                // Marks the enclosing \footnote as an endnote.
                if let Some(frame) = self.dest.note_frames.last_mut() {
                    frame.kind = NoteKind::Endnote;
                }
            }
            "chftn" => {}
            "bkmkstart" => {
                self.flush_pending();
                self.state.capture = Capture::Bookmark;
                self.state.suppress = false;
            }
            // `{\*\shppict {\pict ...}}` wraps the preferred picture; the
            // `\nonshppict` fallback duplicate stays suppressed.
            "shppict" => self.state.suppress = false,
            // `\shpinst` itself is suppressed (shape properties), but its
            // `\shptxt` destination holds the shape's real text.
            "shptxt" => self.state.suppress = false,
            // Likewise `\object` is suppressed (class names, `\objdata`
            // payload), but `\result` is the object's displayable rendering.
            "result" => self.state.suppress = false,
            "pict" => {
                // A pict inside a suppressed destination (the nonshppict
                // fallback, excluded headers) is not extracted.
                if !self.state.suppress {
                    self.flush_pending();
                    self.state.capture = Capture::Pict;
                    self.dest.pict =
                        Some(PictState { depth: self.stack.len(), ..PictState::default() });
                }
            }
            "pngblip" => self.set_pict_format(Some(("image/png", "png"))),
            "jpegblip" => self.set_pict_format(Some(("image/jpeg", "jpg"))),
            "emfblip" => self.set_pict_format(Some(("image/emf", "emf"))),
            "wmetafile" => self.set_pict_format(Some(("image/wmf", "wmf"))),
            "macpict" | "dibitmap" | "wbitmap" => self.set_pict_format(None),
            _ => return false,
        }
        true
    }

    fn set_pict_format(&mut self, format: Option<(&'static str, &'static str)>) {
        if self.state.capture == Capture::Pict
            && let Some(pict) = &mut self.dest.pict
        {
            pict.format = format;
        }
    }

    /// Finalize a closed `\pict` destination: retain the payload as an
    /// asset and emit its inline reference. Unsupported formats degrade
    /// with a log.
    fn finish_pict(&mut self) -> Result<(), ConvertError> {
        let Some(pict) = self.dest.pict.take() else {
            return Ok(());
        };
        let Some((media_type, extension)) = pict.format else {
            log::debug!("skipping picture in an unsupported format");
            return Ok(());
        };
        let bytes = pict.payload();
        if bytes.is_empty() {
            return Ok(());
        }
        let part = format!("pict/{}.{extension}", self.assets.assets.len());
        let id = self.assets.add(media_type.to_string(), part, &bytes)?;
        self.inlines.push(Inline::Image {
            alt: String::new(),
            source: crate::model::ImageSource::Asset(id),
        });
        Ok(())
    }

    /// Table controls act outside suppressed groups and note bodies.
    fn table_active(&self) -> bool {
        !self.state.suppress && self.state.note.is_none()
    }

    fn pending_cell_prop(&mut self, f: impl FnOnce(&mut table::CellProp)) {
        if self.table_active() {
            let depth = self.state.itap.max(1);
            f(self.table.pending_prop(depth));
        }
    }

    fn set_vert_align(&mut self, vert_align: VertAlign) {
        self.set_style(|s| s.vert_align = vert_align);
    }

    fn set_style(&mut self, f: impl FnOnce(&mut Style)) {
        self.flush_pending();
        f(&mut self.state.style);
    }

    fn push_char(&mut self, c: char) {
        if !self.accepts_text() {
            return;
        }
        if self.decoder.skip_char() {
            return;
        }
        self.flush_pending();
        self.push_text(c.to_string());
    }

    fn flush_pending(&mut self) {
        let encoding = self.state.font.and_then(|f| self.prelude.fonts.get(&f).copied());
        if let Some(text) = self.decoder.take_pending(encoding) {
            self.push_text(text);
        }
    }

    fn push_text(&mut self, text: String) {
        let text = clean_text(&text);
        if text.is_empty() {
            return;
        }
        match self.state.capture {
            Capture::ListText => {
                if let Some(lt) = &mut self.dest.listtext {
                    lt.push_str(&text);
                }
            }
            Capture::FieldInstr => {
                if let Some(f) = self.dest.fields.last_mut() {
                    f.instr.push_str(&text);
                }
            }
            Capture::Bookmark => self.dest.bookmark.push_str(&text),
            // Picture payload bytes are collected raw in the token loop.
            Capture::Pict => {}
            Capture::None => {
                if !self.state.suppress {
                    self.inlines.push(Inline::Text { text, style: self.state.style });
                }
            }
        }
    }

    /// Flush the finished top-level table, if any, into the block stream.
    fn flush_top_table(&mut self) -> Result<(), ConvertError> {
        if let Some(block) = self.table.take_table(1)? {
            self.flush_runs();
            self.blocks.push(block);
        }
        Ok(())
    }

    fn end_paragraph(&mut self) -> Result<(), ConvertError> {
        let inlines = std::mem::take(&mut self.inlines);
        let listtext = self.dest.listtext.take();

        if self.state.in_table {
            let depth = self.state.itap.max(1);
            self.table.push_cell_paragraph(depth, self.state.block, inlines);
            return Ok(());
        }
        self.flush_top_table()?;

        // A styled container absorbs its blank paragraphs: they are the
        // blank lines of a code block.
        if let Some(style) = self.state.block {
            flush_list(&mut self.blocks, &mut self.list_run);
            self.styled.push(style, inlines, &mut self.blocks);
            return Ok(());
        }
        if inlines_are_empty(&inlines) {
            self.flush_runs();
            return Ok(());
        }
        // Numbering identity comes from the list tables; the captured label
        // text only seeds legacy Word-95 numbering. Numbered headings
        // advance the sequence and keep their number visible.
        let entry = self.list_entry(listtext.as_deref());
        if let Some(level) = self.state.outline {
            self.flush_runs();
            let mut content = inlines;
            rebase_emphasis(&mut content, self.state.style_base);
            if let Some((key, _, number, label)) = &entry
                && key.marker.ordered()
            {
                let text =
                    format!("{} ", label.clone().unwrap_or_else(|| key.marker.label(*number)));
                content.insert(0, Inline::Text { text, style: Style::PLAIN });
            }
            self.blocks.push(Block::Heading { level, anchor: None, content });
            return Ok(());
        }
        if let Some((key, level, number, label)) = entry {
            self.list_run.push(ListEntry {
                level,
                key,
                number,
                label,
                blocks: vec![Block::Paragraph(inlines)],
            });
            return Ok(());
        }
        self.flush_runs();
        self.blocks.push(Block::Paragraph(inlines));
        Ok(())
    }

    /// A captured `\listtext` as a literal marker label: trimmed, non-empty.
    fn trimmed_label(listtext: Option<&str>) -> Option<String> {
        listtext.map(str::trim).filter(|t| !t.is_empty()).map(str::to_string)
    }

    #[allow(clippy::type_complexity)]
    fn list_entry(
        &mut self,
        listtext: Option<&str>,
    ) -> Option<(ListKey, usize, u64, Option<String>)> {
        if let Some(ls) = self.state.ls {
            let level = self.state.ilvl;
            if let Some(list) = self.prelude.lists.get(&ls) {
                let def = &list.levels[level.min(LIST_LEVELS - 1)];
                let marker = def.marker?;
                let (number, label) = if marker.ordered() {
                    self.counters.next_labeled(ls, level, &list.levels)
                } else {
                    (0, None)
                };
                return Some((ListKey { instance: ls as u64, marker }, level, number, label));
            }
            // No table definition behind the \ls: the captured \listtext is
            // the only surviving evidence of the real marker; carry it as
            // the item's literal label over a bullet base.
            return Some((
                ListKey { instance: ls as u64, marker: MarkerKind::Bullet },
                level,
                0,
                Self::trimmed_label(listtext),
            ));
        }
        if let Some(marker) = self.state.legacy_list {
            let number = if marker.ordered() {
                // Legacy \pn numbering: the label text carries the number,
                // clamped so a crafted label cannot overflow the counter.
                let parsed = listtext.and_then(|t| {
                    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
                    digits.parse::<u64>().ok().map(|n| n.min(u32::MAX as u64))
                });
                match parsed {
                    Some(n) => {
                        self.counters.seed(i32::MAX, self.state.ilvl, n);
                        n
                    }
                    None => self.counters.next(i32::MAX, self.state.ilvl, 1),
                }
            } else {
                0
            };
            return Some((ListKey { instance: u64::MAX, marker }, self.state.ilvl, number, None));
        }
        // A bare \listtext with no list state still marks a list paragraph
        // (some producers omit \ls); its text is the literal marker.
        if listtext.is_some_and(|t| !t.trim().is_empty()) {
            return Some((
                ListKey { instance: u64::MAX - 1, marker: MarkerKind::Bullet },
                self.state.ilvl,
                0,
                Self::trimmed_label(listtext),
            ));
        }
        None
    }

    fn end_cell(&mut self, depth: usize) -> Result<(), ConvertError> {
        let inlines = std::mem::take(&mut self.inlines);
        self.dest.listtext = None;
        self.table.end_cell(depth, self.state.block, inlines)
    }

    fn end_row(&mut self, depth: usize) -> Result<(), ConvertError> {
        if self.table.has_pending_cell(depth) || !inlines_are_empty(&self.inlines) {
            self.end_cell(depth)?;
        }
        self.table.end_row(depth);
        Ok(())
    }

    /// Close every open block run before something else is emitted.
    fn flush_runs(&mut self) {
        self.styled.flush(&mut self.blocks);
        flush_list(&mut self.blocks, &mut self.list_run);
    }

    fn finish(mut self) -> Result<Document, ConvertError> {
        for depth in (1..=self.table.depth()).rev() {
            if self.table.has_partial_row(depth) {
                self.end_row(depth)?;
            }
        }
        // Collapse any dangling nested tables outward, then flush.
        self.table.collapse_nested()?;
        self.flush_top_table()?;
        self.flush_runs();
        Ok(Document { blocks: self.blocks, notes: self.dest.notes, assets: self.assets.assets })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_list_table_keeps_listtext_marker() {
        // A \ls with no list-table definition degrades to a bullet, but the
        // captured \listtext (the marker Word displays) must stay visible.
        let doc = parse(
            br"{\rtf1 \pard{\listtext 1.\tab}\ls5 one\par \pard{\listtext 2.\tab}\ls5 two\par}",
        )
        .unwrap();
        let Block::List(list) = &doc.blocks[0] else { panic!("{:?}", doc.blocks) };
        assert_eq!(list.items[0].marker_label.as_deref(), Some("1."));
        assert_eq!(list.items[1].marker_label.as_deref(), Some("2."));
    }

    #[test]
    fn scripts_survive_as_scripts() {
        let src = r"{\rtf1 H\sub 2\nosupersub O and 10\super -3\nosupersub  mol\par}";
        let markdown = crate::to_markdown_bytes(src.as_bytes(), crate::Format::Rtf).unwrap();
        assert_eq!(markdown, "H<sub>2</sub>O and 10<sup>-3</sup> mol\n");
    }

    #[test]
    fn a_half_point_offset_says_which_way_and_zero_says_neither() {
        // \upN and \dnN carry an offset, not a toggle: only 0 is the baseline.
        let src = r"{\rtf1 a\up6 b\up0 c\dn4 d\plain e\par}";
        let markdown = crate::to_markdown_bytes(src.as_bytes(), crate::Format::Rtf).unwrap();
        assert_eq!(markdown, "a<sup>b</sup>c<sub>d</sub>e\n");
    }

    #[test]
    fn a_paragraph_style_can_carry_the_script() {
        let src = r"{\rtf1{\stylesheet{\s15\super Raised;}}\pard\s15 note\par}";
        let markdown = crate::to_markdown_bytes(src.as_bytes(), crate::Format::Rtf).unwrap();
        assert_eq!(markdown, "<sup>note</sup>\n");
    }

    #[test]
    fn mid_paragraph_page_and_column_breaks_keep_the_word_boundary() {
        // \page and \column carry no paragraph mark: without a break of
        // their own the text on either side would run together.
        for src in [r"{\rtf1 Alfa\page Beta\par}", r"{\rtf1 Alfa\column Beta\par}"] {
            let markdown = crate::to_markdown_bytes(src.as_bytes(), crate::Format::Rtf).unwrap();
            assert_eq!(markdown, "Alfa\\\nBeta\n", "source: {src}");
        }
    }

    #[test]
    fn backslash_before_a_line_break_breaks_the_paragraph() {
        // The paragraph mark Cocoa's RTF writer emits instead of `\par`.
        for src in
            ["{\\rtf1 Alpha\\\nBeta\\\nGamma\\\n}", "{\\rtf1 Alpha\\\r\nBeta\\\rGamma\\\r\n}"]
        {
            let markdown = crate::to_markdown_bytes(src.as_bytes(), crate::Format::Rtf).unwrap();
            assert_eq!(markdown, "Alpha\n\nBeta\n\nGamma\n", "source: {src:?}");
        }
    }

    #[test]
    fn body_text_before_a_table_stays_out_of_the_first_cell() {
        let src = "{\\rtf1 Intro\\\n\\trowd\\cellx2000\\cellx4000 A\\cell B\\cell\\row}";
        let markdown = crate::to_markdown_bytes(src.as_bytes(), crate::Format::Rtf).unwrap();
        assert!(markdown.starts_with("Intro\n\n|"), "{markdown}");
    }

    #[test]
    fn styled_runs_stop_before_tables_and_work_inside_cells() {
        let src = br"{\rtf1\ansi{\stylesheet{\s0 Normal;}{\s1 Source Code;}}\pard\plain\s1 before table\par\trowd\cellx2000\pard\plain\intbl\s1 first\par second\cell\row}";
        let doc = parse(src).unwrap();
        let [Block::CodeBlock { text: before, .. }, Block::Table(table)] = &doc.blocks[..] else {
            panic!("unexpected block order: {:?}", doc.blocks)
        };
        assert_eq!(before, "before table");
        let crate::model::CellSlot::Origin(cell) = &table.grid[0][0] else {
            panic!("expected an origin cell")
        };
        let [Block::CodeBlock { text: inside, .. }] = &cell.blocks[..] else {
            panic!("unexpected cell blocks: {:?}", cell.blocks)
        };
        assert_eq!(inside, "first\nsecond");
    }

    #[test]
    fn pict_hex_payload_becomes_an_asset() {
        let doc = parse(br"{\rtf1 before {\pict\pngblip 89504e470d0a1a0a} after}").unwrap();
        assert_eq!(doc.assets.len(), 1, "assets: {:?}", doc.assets);
        assert_eq!(doc.assets[0].media_type, "image/png");
        assert!(doc.assets[0].bytes.starts_with(&[0x89, b'P', b'N', b'G']));
    }

    #[test]
    fn pict_property_subgroups_do_not_contaminate_the_payload() {
        let doc = parse(
            br"{\rtf1 {\pict{\*\picprop{\sp{\sn wzDescription}{\sv abcdef}}}\pngblip 89504e470d0a1a0a}}",
        )
        .unwrap();
        assert_eq!(doc.assets.len(), 1, "assets: {:?}", doc.assets);
        assert!(doc.assets[0].bytes.starts_with(&[0x89, b'P', b'N', b'G']));
    }

    #[test]
    fn nonshppict_fallback_is_not_extracted_twice() {
        let doc = parse(
            br"{\rtf1 {\*\shppict{\pict\pngblip 89504e47}}{\nonshppict{\pict\wmetafile8 0102}}}",
        )
        .unwrap();
        assert_eq!(doc.assets.len(), 1, "only the preferred picture: {:?}", doc.assets);
        assert_eq!(doc.assets[0].media_type, "image/png");
    }
}
