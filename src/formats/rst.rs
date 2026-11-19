//! reStructuredText (.rst) frontend parser.

use crate::error::ConvertError;
use crate::model::{
    Block, Cell, Document, ImageSource, Inline, LinkTarget, List, ListItem, MarkerKind,
    Note, NoteKind, Style, Table, TableKind,
};
use std::collections::HashMap;

pub fn parse(bytes: &[u8]) -> Result<Document, ConvertError> {
    let text = decode(bytes);
    let mut parser = RstParser::new(&text);
    Ok(parser.parse_document())
}

fn decode(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8_lossy(bytes).into_owned()
}

struct RstParser<'a> {
    lines: Vec<&'a str>,
    substitutions: HashMap<String, String>,
    heading_levels: Vec<(Option<char>, char)>, // (overline, underline)
    notes: Vec<Note>,
}

impl<'a> RstParser<'a> {
    fn new(input: &'a str) -> Self {
        let lines: Vec<&'a str> = input.lines().collect();
        Self {
            lines,
            substitutions: HashMap::new(),
            heading_levels: Vec::new(),
            notes: Vec::new(),
        }
    }

    fn parse_document(&mut self) -> Document {
        // Pre-pass: collect substitutions, footnote definitions, citations
        self.collect_prepass();

        let mut doc = Document::default();
        let blocks = self.parse_block_range(0, self.lines.len(), 0);
        doc.blocks = blocks;
        doc.notes = std::mem::take(&mut self.notes);
        doc
    }

    fn collect_prepass(&mut self) {
        let mut i = 0;
        while i < self.lines.len() {
            let line = self.lines[i].trim();
            // Substitution: .. |name| replace:: value
            if line.starts_with(".. |") {
                if let Some(rest) = line.strip_prefix(".. |") {
                    if let Some(pipe_pos) = rest.find('|') {
                        let name = &rest[..pipe_pos];
                        let after = rest[pipe_pos + 1..].trim();
                        if let Some(val) = after.strip_prefix("replace::") {
                            self.substitutions
                                .insert(name.to_string(), val.trim().to_string());
                        }
                    }
                }
            }
            // Footnote definition: .. [#id] text or .. [id] text
            else if line.starts_with(".. [") {
                if let Some(close_bracket) = line.find(']') {
                    let raw_id = &line[4..close_bracket];
                    let id = raw_id.trim_start_matches('#').to_string();
                    let content_start = line[close_bracket + 1..].trim();
                    let mut note_text = content_start.to_string();
                    let indent = self.line_indent(self.lines[i]);
                    i += 1;
                    while i < self.lines.len() {
                        let next_line = self.lines[i];
                        if next_line.trim().is_empty() {
                            i += 1;
                            break;
                        }
                        if self.line_indent(next_line) > indent {
                            if !note_text.is_empty() {
                                note_text.push(' ');
                            }
                            note_text.push_str(next_line.trim());
                            i += 1;
                        } else {
                            break;
                        }
                    }
                    let blocks = vec![Block::Paragraph(self.parse_inlines(&note_text))];
                    self.notes.push(Note {
                        id,
                        kind: NoteKind::Footnote,
                        blocks,
                    });
                    continue;
                }
            }
            i += 1;
        }
    }

    fn parse_block_range(&mut self, start: usize, end: usize, min_indent: usize) -> Vec<Block> {
        let mut blocks = Vec::new();
        let mut i = start;

        while i < end {
            let line = self.lines[i];
            let trimmed = line.trim();

            if trimmed.is_empty() {
                i += 1;
                continue;
            }

            let indent = self.line_indent(line);
            if indent < min_indent && !trimmed.is_empty() {
                // Outer scope block end
                break;
            }

            // 1. Heading checks (Overline or Underline)
            if let Some((level, title, next_i)) = self.try_parse_heading(i, end) {
                let inlines = self.parse_inlines(&title);
                blocks.push(Block::Heading {
                    level,
                    anchor: None,
                    content: inlines,
                });
                i = next_i;
                continue;
            }

            // 2. Horizontal Rule (----)
            if self.is_adornment_line(trimmed) && trimmed.len() >= 4 && !self.is_table_border(trimmed) {
                // Check if preceded and followed by blank line or start/end
                blocks.push(Block::Rule);
                i += 1;
                continue;
            }

            // 3. Grid Table
            if self.is_table_border(trimmed) {
                if let Some((table_block, next_i)) = self.try_parse_grid_table(i, end) {
                    blocks.push(table_block);
                    i = next_i;
                    continue;
                }
            }

            // 4. Directives (.. directive::)
            if trimmed.starts_with(".. ") {
                if let Some((directive_blocks, next_i)) = self.try_parse_directive(i, end) {
                    blocks.extend(directive_blocks);
                    i = next_i;
                    continue;
                }
            }

            // 5. Line blocks (| Line 1)
            if trimmed.starts_with("| ") {
                let mut line_inlines = Vec::new();
                while i < end {
                    let l = self.lines[i].trim();
                    if let Some(content) = l.strip_prefix("| ") {
                        if !line_inlines.is_empty() {
                            line_inlines.push(Inline::LineBreak);
                        }
                        line_inlines.extend(self.parse_inlines(content.trim()));
                        i += 1;
                    } else {
                        break;
                    }
                }
                blocks.push(Block::Paragraph(line_inlines));
                continue;
            }

            // 6. Field lists (:Author: Jane Doe)
            if trimmed.starts_with(':') && trimmed[1..].contains(':') {
                if let Some((field_blocks, next_i)) = self.try_parse_field_list(i, end) {
                    blocks.extend(field_blocks);
                    i = next_i;
                    continue;
                }
            }

            // 7. Bullet Lists & Enumerated Lists
            if self.is_list_item_start(trimmed) {
                if let Some((list_block, next_i)) = self.try_parse_list(i, end, min_indent) {
                    blocks.push(list_block);
                    i = next_i;
                    continue;
                }
            }

            // 8. Definition List (term line followed by indented paragraph)
            if self.is_definition_list_start(i, end) {
                if let Some((def_blocks, next_i)) = self.try_parse_definition_list(i, end) {
                    blocks.extend(def_blocks);
                    i = next_i;
                    continue;
                }
            }

            // 9. Indented Blockquote (if indented more than min_indent)
            if indent > min_indent && !self.is_list_item_start(trimmed) {
                let (bq_blocks, next_i) = self.parse_blockquote(i, end, indent);
                blocks.push(Block::BlockQuote(bq_blocks));
                i = next_i;
                continue;
            }

            // 10. Paragraph (and check for trailing :: literal block trigger)
            let (para_block, next_i, has_literal_next) = self.parse_paragraph(i, end);
            blocks.push(para_block);
            i = next_i;

            if has_literal_next && i < end {
                // Parse subsequent indented literal block
                let (code_block, next_code_i) = self.parse_literal_block(i, end);
                blocks.push(code_block);
                i = next_code_i;
            }
        }

        blocks
    }

    fn line_indent(&self, line: &str) -> usize {
        line.chars().take_while(|c| c.is_whitespace()).count()
    }

    fn is_adornment_char(c: char) -> bool {
        matches!(
            c,
            '=' | '-' | '~' | '^' | '"' | '\'' | '#' | '*' | '+' | ':' | '<' | '>'
        )
    }

    fn is_adornment_line(&self, line: &str) -> bool {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return false;
        }
        let first = trimmed.chars().next().unwrap();
        Self::is_adornment_char(first) && trimmed.chars().all(|c| c == first)
    }

    fn is_table_border(&self, line: &str) -> bool {
        let trimmed = line.trim();
        trimmed.starts_with('+') && trimmed.ends_with('+') && trimmed.contains('-')
    }

    fn get_heading_level(&mut self, overline: Option<char>, underline: char) -> u8 {
        let pair = (overline, underline);
        if let Some(pos) = self.heading_levels.iter().position(|p| *p == pair) {
            (pos + 1).min(6) as u8
        } else {
            self.heading_levels.push(pair);
            self.heading_levels.len().min(6) as u8
        }
    }

    fn try_parse_heading(&mut self, i: usize, end: usize) -> Option<(u8, String, usize)> {
        let line = self.lines[i].trim_end();
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            return None;
        }

        // Case 1: Overline + Title + Underline
        if self.is_adornment_line(trimmed) && i + 2 < end {
            let overline_char = trimmed.chars().next().unwrap();
            let title_line = self.lines[i + 1].trim();
            let underline_line = self.lines[i + 2].trim();
            if !title_line.is_empty()
                && self.is_adornment_line(underline_line)
                && underline_line.chars().next().unwrap() == overline_char
            {
                let level = self.get_heading_level(Some(overline_char), overline_char);
                return Some((level, title_line.to_string(), i + 3));
            }
        }

        // Case 2: Title + Underline
        if i + 1 < end {
            let title_line = trimmed;
            let next_trimmed = self.lines[i + 1].trim();
            if !title_line.is_empty()
                && !self.is_adornment_line(title_line)
                && self.is_adornment_line(next_trimmed)
                && next_trimmed.len() >= title_line.len().saturating_sub(4)
            {
                let underline_char = next_trimmed.chars().next().unwrap();
                let level = self.get_heading_level(None, underline_char);
                return Some((level, title_line.to_string(), i + 2));
            }
        }

        None
    }

    fn try_parse_grid_table(&self, start: usize, end: usize) -> Option<(Block, usize)> {
        let mut i = start;
        let mut table_lines = Vec::new();

        while i < end {
            let line = self.lines[i].trim();
            if line.starts_with('+') || line.starts_with('|') {
                table_lines.push(line);
                i += 1;
            } else {
                break;
            }
        }

        if table_lines.len() < 3 {
            return None;
        }

        // Parse grid table lines
        let mut rows: Vec<Vec<String>> = Vec::new();
        let mut header_rows = 0;
        let mut current_row_cells: Vec<String> = Vec::new();

        for (_idx, line) in table_lines.iter().enumerate() {
            if line.starts_with('+') {
                if line.contains('=') {
                    header_rows = rows.len();
                }
                if !current_row_cells.is_empty() {
                    rows.push(std::mem::take(&mut current_row_cells));
                }
            } else if line.starts_with('|') {
                let cell_texts: Vec<String> = line
                    .split('|')
                    .skip(1)
                    .take_while(|_| true)
                    .map(|s| s.trim().to_string())
                    .collect();
                // Filter out empty trailing split result
                let cell_texts: Vec<String> = if cell_texts.last().map_or(false, |s| s.is_empty())
                    && cell_texts.len() > 1
                {
                    cell_texts[..cell_texts.len() - 1].to_vec()
                } else {
                    cell_texts
                };

                if current_row_cells.is_empty() {
                    current_row_cells = cell_texts;
                } else {
                    // Multiline cell content continuation
                    for (c_idx, text) in cell_texts.iter().enumerate() {
                        if c_idx < current_row_cells.len() {
                            if !text.is_empty() {
                                if !current_row_cells[c_idx].is_empty() {
                                    current_row_cells[c_idx].push(' ');
                                }
                                current_row_cells[c_idx].push_str(text);
                            }
                        } else {
                            current_row_cells.push(text.clone());
                        }
                    }
                }
            }
        }

        if !current_row_cells.is_empty() {
            rows.push(current_row_cells);
        }

        if rows.is_empty() {
            return None;
        }

        let mut cell_rows: Vec<Vec<Cell>> = Vec::new();
        for row in rows {
            let cells: Vec<Cell> = row
                .into_iter()
                .map(|text| Cell::from_inlines(self.parse_inlines_static(&text)))
                .collect();
            if !cells.is_empty() {
                cell_rows.push(cells);
            }
        }

        let mut table = Table::from_rows(cell_rows, header_rows, TableKind::Data);
        table.header_rows = header_rows;
        Some((Block::Table(table), i))
    }

    fn try_parse_directive(&mut self, start: usize, end: usize) -> Option<(Vec<Block>, usize)> {
        let first_line = self.lines[start].trim();
        let directive_head = first_line.strip_prefix(".. ")?;
        let colon_pos = directive_head.find("::")?;
        let name = directive_head[..colon_pos].trim();
        let arg = directive_head[colon_pos + 2..].trim();

        let base_indent = self.line_indent(self.lines[start]);
        let mut i = start + 1;

        // Skip options (:option: val)
        let mut options = HashMap::new();
        while i < end {
            let l = self.lines[i];
            let t = l.trim();
            if t.starts_with(':') && t[1..].contains(':') {
                let rest = &t[1..];
                if let Some(c_pos) = rest.find(':') {
                    let opt_key = rest[..c_pos].trim();
                    let opt_val = rest[c_pos + 1..].trim();
                    options.insert(opt_key.to_string(), opt_val.to_string());
                }
                i += 1;
            } else if t.is_empty() {
                i += 1;
                break;
            } else {
                break;
            }
        }

        // Collect body block (indented lines)
        let body_start = i;
        let mut body_indent = 0;
        while i < end {
            let l = self.lines[i];
            if l.trim().is_empty() {
                i += 1;
                continue;
            }
            let ind = self.line_indent(l);
            if ind > base_indent {
                if body_indent == 0 {
                    body_indent = ind;
                }
                i += 1;
            } else {
                break;
            }
        }
        let body_end = i;

        let blocks = match name {
            "code-block" | "code" => {
                let lang = if !arg.is_empty() {
                    Some(arg.to_string())
                } else {
                    None
                };
                let mut code_text = String::new();
                for line_idx in body_start..body_end {
                    let line = self.lines[line_idx];
                    let line_stripped = if line.len() >= body_indent {
                        &line[body_indent..]
                    } else {
                        line.trim()
                    };
                    code_text.push_str(line_stripped);
                    code_text.push('\n');
                }
                vec![Block::CodeBlock {
                    lang,
                    text: code_text,
                }]
            }
            "image" | "figure" => {
                let alt = options.get("alt").cloned().unwrap_or_default();
                let source = ImageSource::External(arg.to_string());
                vec![Block::Paragraph(vec![Inline::Image { alt, source }])]
            }
            "note" | "warning" | "important" | "tip" | "caution" | "danger" | "error" | "hint"
            | "admonition" => {
                let title_text = if name == "admonition" && !arg.is_empty() {
                    arg.to_string()
                } else {
                    let mut c = name.chars();
                    match c.next() {
                        None => String::new(),
                        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                    }
                };

                let mut bq_inner = vec![Block::Paragraph(vec![Inline::Text {
                    text: title_text,
                    style: Style {
                        bold: true,
                        italic: false,
                        strike: false,
                        code: false,
                    },
                }])];

                if body_start < body_end {
                    let inner_blocks = self.parse_block_range(body_start, body_end, body_indent);
                    bq_inner.extend(inner_blocks);
                }

                vec![Block::BlockQuote(bq_inner)]
            }
            "list-table" => {
                let header_rows: usize = options
                    .get("header-rows")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                let table = self.parse_list_table(body_start, body_end, header_rows);
                vec![Block::Table(table)]
            }
            "container" => {
                if body_start < body_end {
                    self.parse_block_range(body_start, body_end, body_indent)
                } else {
                    vec![]
                }
            }
            "raw" => {
                let mut raw_text = String::new();
                for line_idx in body_start..body_end {
                    let line = self.lines[line_idx];
                    let line_stripped = if line.len() >= body_indent {
                        &line[body_indent..]
                    } else {
                        line.trim()
                    };
                    raw_text.push_str(line_stripped);
                    raw_text.push('\n');
                }
                vec![Block::CodeBlock {
                    lang: if !arg.is_empty() { Some(arg.to_string()) } else { None },
                    text: raw_text,
                }]
            }
            "contents" => {
                // Table of contents directive
                if !arg.is_empty() {
                    vec![Block::Heading {
                        level: 2,
                        anchor: None,
                        content: vec![Inline::plain(arg)],
                    }]
                } else {
                    vec![]
                }
            }
            _ => {
                // Generic directive fallback: parse body blocks if any
                if body_start < body_end {
                    self.parse_block_range(body_start, body_end, body_indent)
                } else if !arg.is_empty() {
                    vec![Block::Paragraph(self.parse_inlines(arg))]
                } else {
                    vec![]
                }
            }
        };

        Some((blocks, i))
    }

    fn parse_list_table(&mut self, start: usize, end: usize, header_rows: usize) -> Table {
        let mut rows: Vec<Vec<Cell>> = Vec::new();
        let mut current_row: Vec<Cell> = Vec::new();
        let mut current_cell_text = String::new();

        let mut i = start;
        while i < end {
            let line = self.lines[i].trim();
            if line.starts_with("* - ") || line == "*" {
                if !current_cell_text.is_empty() {
                    current_row.push(Cell::from_inlines(self.parse_inlines(&current_cell_text)));
                    current_cell_text.clear();
                }
                if !current_row.is_empty() {
                    rows.push(std::mem::take(&mut current_row));
                }
                if let Some(content) = line.strip_prefix("* - ") {
                    current_cell_text.push_str(content.trim());
                }
            } else if line.starts_with("- ") {
                if !current_cell_text.is_empty() {
                    current_row.push(Cell::from_inlines(self.parse_inlines(&current_cell_text)));
                    current_cell_text.clear();
                }
                current_cell_text.push_str(line[2..].trim());
            } else if !line.is_empty() {
                if !current_cell_text.is_empty() {
                    current_cell_text.push(' ');
                }
                current_cell_text.push_str(line);
            }
            i += 1;
        }

        if !current_cell_text.is_empty() {
            current_row.push(Cell::from_inlines(self.parse_inlines(&current_cell_text)));
        }
        if !current_row.is_empty() {
            rows.push(current_row);
        }

        let mut table = Table::from_rows(rows, header_rows, TableKind::Data);
        table.header_rows = header_rows;
        table
    }

    fn try_parse_field_list(&mut self, start: usize, end: usize) -> Option<(Vec<Block>, usize)> {
        let mut i = start;
        let mut items = Vec::new();

        while i < end {
            let line = self.lines[i].trim();
            if line.starts_with(':') && line[1..].contains(':') {
                let rest = &line[1..];
                if let Some(colon_pos) = rest.find(':') {
                    let field_name = rest[..colon_pos].trim();
                    let field_val = rest[colon_pos + 1..].trim();
                    let label = format!("{field_name}: ");
                    let mut content_inlines = vec![Inline::Text {
                        text: label,
                        style: Style {
                            bold: true,
                            italic: false,
                            strike: false,
                            code: false,
                        },
                    }];
                    content_inlines.extend(self.parse_inlines(field_val));

                    items.push(ListItem {
                        blocks: vec![Block::Paragraph(content_inlines)],
                        marker_label: None,
                        checked: None,
                    });
                }
                i += 1;
            } else {
                break;
            }
        }

        if items.is_empty() {
            return None;
        }

        let list = List {
            items,
            marker: MarkerKind::Bullet,
            start: 1,
        };
        Some((vec![Block::List(list)], i))
    }

    fn is_list_item_start(&self, line: &str) -> bool {
        let trimmed = line.trim();
        if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("+ ") {
            return true;
        }
        if trimmed.starts_with("#. ") {
            return true;
        }
        if let Some(first_word) = trimmed.split_whitespace().next() {
            if first_word.ends_with('.') && first_word.len() <= 4 {
                let num_part = &first_word[..first_word.len() - 1];
                if num_part.chars().all(|c| c.is_ascii_digit()) {
                    return true;
                }
            }
        }
        false
    }

    fn try_parse_list(&mut self, start: usize, end: usize, min_indent: usize) -> Option<(Block, usize)> {
        let first_line = self.lines[start].trim();
        let (marker_kind, marker_len) = if first_line.starts_with("#. ") {
            (MarkerKind::Decimal, 3)
        } else if first_line.starts_with("- ") || first_line.starts_with("* ") || first_line.starts_with("+ ") {
            (MarkerKind::Bullet, 2)
        } else {
            (MarkerKind::Decimal, 3)
        };

        let base_indent = self.line_indent(self.lines[start]);
        let mut items = Vec::new();
        let mut i = start;

        while i < end {
            let line = self.lines[i];
            let trimmed = line.trim();
            if trimmed.is_empty() {
                i += 1;
                continue;
            }

            let ind = self.line_indent(line);
            if ind < min_indent {
                break;
            }

            if self.is_list_item_start(trimmed) && ind == base_indent {
                let content_start_line = if trimmed.starts_with("- ")
                    || trimmed.starts_with("* ")
                    || trimmed.starts_with("+ ")
                {
                    &trimmed[2..]
                } else if trimmed.starts_with("#. ") {
                    &trimmed[3..]
                } else {
                    let pos = trimmed.find(' ').unwrap_or(0);
                    &trimmed[pos..]
                };

                let mut item_lines = vec![content_start_line.trim()];
                let item_indent = base_indent + marker_len;
                i += 1;

                while i < end {
                    let next_line = self.lines[i];
                    if next_line.trim().is_empty() {
                        let mut lookahead = i + 1;
                        while lookahead < end && self.lines[lookahead].trim().is_empty() {
                            lookahead += 1;
                        }
                        if lookahead < end {
                            let next_ind = self.line_indent(self.lines[lookahead]);
                            if next_ind >= item_indent && !self.is_list_item_start(self.lines[lookahead].trim()) {
                                i = lookahead;
                                continue;
                            }
                        }
                        break;
                    }

                    let next_ind = self.line_indent(next_line);
                    if next_ind >= item_indent || (!self.is_list_item_start(next_line.trim()) && next_ind > base_indent) {
                        item_lines.push(next_line.trim());
                        i += 1;
                    } else {
                        break;
                    }
                }

                let item_text = item_lines.join("\n");
                let item_blocks = vec![Block::Paragraph(self.parse_inlines(&item_text))];
                items.push(ListItem {
                    blocks: item_blocks,
                    marker_label: None,
                    checked: None,
                });
            } else {
                break;
            }
        }

        if items.is_empty() {
            return None;
        }

        let list = List {
            items,
            marker: marker_kind,
            start: 1,
        };
        Some((Block::List(list), i))
    }

    fn is_definition_list_start(&self, i: usize, end: usize) -> bool {
        if i + 1 >= end {
            return false;
        }
        let term_line = self.lines[i].trim();
        let def_line = self.lines[i + 1];
        if term_line.is_empty() || self.is_adornment_line(term_line) || term_line.starts_with(".. ") {
            return false;
        }
        let term_indent = self.line_indent(self.lines[i]);
        let def_indent = self.line_indent(def_line);
        def_indent > term_indent && !def_line.trim().is_empty()
    }

    fn try_parse_definition_list(&mut self, start: usize, end: usize) -> Option<(Vec<Block>, usize)> {
        let term = self.lines[start].trim();
        let _term_indent = self.line_indent(self.lines[start]);
        let def_indent = self.line_indent(self.lines[start + 1]);

        let mut def_lines = Vec::new();
        let mut i = start + 1;
        while i < end {
            let line = self.lines[i];
            if line.trim().is_empty() {
                i += 1;
                continue;
            }
            if self.line_indent(line) >= def_indent {
                def_lines.push(line.trim());
                i += 1;
            } else {
                break;
            }
        }

        let mut term_inlines = vec![Inline::Text {
            text: term.to_string(),
            style: Style {
                bold: true,
                italic: false,
                strike: false,
                code: false,
            },
        }];
        term_inlines.push(Inline::LineBreak);
        term_inlines.extend(self.parse_inlines(&def_lines.join(" ")));

        Some((vec![Block::Paragraph(term_inlines)], i))
    }

    fn parse_blockquote(&mut self, start: usize, end: usize, indent: usize) -> (Vec<Block>, usize) {
        let mut bq_lines = Vec::new();
        let mut i = start;
        while i < end {
            let line = self.lines[i];
            if line.trim().is_empty() {
                i += 1;
                continue;
            }
            if self.line_indent(line) >= indent {
                let stripped = if line.len() >= indent {
                    &line[indent..]
                } else {
                    line.trim()
                };
                bq_lines.push(stripped);
                i += 1;
            } else {
                break;
            }
        }

        let inner_text = bq_lines.join("\n");
        let inner_parser_lines: Vec<&str> = inner_text.lines().collect();
        let mut inner_parser = RstParser {
            lines: inner_parser_lines,
            substitutions: self.substitutions.clone(),
            heading_levels: self.heading_levels.clone(),
            notes: Vec::new(),
        };
        let blocks = inner_parser.parse_block_range(0, inner_parser.lines.len(), 0);
        self.notes.extend(inner_parser.notes);
        (blocks, i)
    }

    fn parse_paragraph(&mut self, start: usize, end: usize) -> (Block, usize, bool) {
        let mut para_lines = Vec::new();
        let mut i = start;

        while i < end {
            let line = self.lines[i];
            let trimmed = line.trim();

            if trimmed.is_empty() {
                i += 1;
                break;
            }

            // Check if line starts next block syntax (heading, table, list, directive, rule)
            if !para_lines.is_empty() {
                if self.is_adornment_line(trimmed)
                    || self.is_table_border(trimmed)
                    || trimmed.starts_with(".. ")
                    || self.is_list_item_start(trimmed)
                {
                    break;
                }
            }

            para_lines.push(trimmed);
            i += 1;
        }

        let full_text = para_lines.join("\n");
        let (clean_text, has_literal_next) = if full_text.ends_with("::") {
            (full_text[..full_text.len() - 2].trim().to_string(), true)
        } else {
            (full_text, false)
        };

        let inlines = self.parse_inlines(&clean_text);
        (Block::Paragraph(inlines), i, has_literal_next)
    }

    fn parse_literal_block(&self, start: usize, end: usize) -> (Block, usize) {
        let mut i = start;
        let mut code_lines = Vec::new();
        let mut indent = 0;

        while i < end {
            let line = self.lines[i];
            if line.trim().is_empty() {
                code_lines.push("");
                i += 1;
                continue;
            }
            let ind = self.line_indent(line);
            if indent == 0 {
                indent = ind;
            }
            if ind >= indent {
                let line_stripped = if line.len() >= indent {
                    &line[indent..]
                } else {
                    line.trim()
                };
                code_lines.push(line_stripped);
                i += 1;
            } else {
                break;
            }
        }

        let code_text = code_lines.join("\n");
        (
            Block::CodeBlock {
                lang: None,
                text: code_text,
            },
            i,
        )
    }

    fn parse_inlines_static(&self, text: &str) -> Vec<Inline> {
        let mut p = RstParser {
            lines: vec![],
            substitutions: self.substitutions.clone(),
            heading_levels: vec![],
            notes: vec![],
        };
        p.parse_inlines(text)
    }

    fn parse_inlines(&mut self, text: &str) -> Vec<Inline> {
        if text.is_empty() {
            return vec![];
        }

        let mut inlines = Vec::new();
        let mut chars = text.chars().peekable();
        let mut buf = String::new();

        while let Some(c) = chars.next() {
            // 1. Double Backticks: Inline Code ``code``
            if c == '`' && chars.peek() == Some(&'`') {
                chars.next(); // consume second `
                if !buf.is_empty() {
                    inlines.push(Inline::plain(std::mem::take(&mut buf)));
                }
                let mut code_buf = String::new();
                while let Some(nc) = chars.next() {
                    if nc == '`' && chars.peek() == Some(&'`') {
                        chars.next();
                        break;
                    }
                    code_buf.push(nc);
                }
                inlines.push(Inline::Text {
                    text: code_buf,
                    style: Style {
                        bold: false,
                        italic: false,
                        strike: false,
                        code: true,
                    },
                });
                continue;
            }

            // 2. Single Backtick: Links `text <url>`_ or ref `target`_
            if c == '`' {
                if !buf.is_empty() {
                    inlines.push(Inline::plain(std::mem::take(&mut buf)));
                }
                let mut link_buf = String::new();
                while let Some(nc) = chars.next() {
                    if nc == '`' {
                        break;
                    }
                    link_buf.push(nc);
                }
                if chars.peek() == Some(&'_') {
                    chars.next(); // consume _
                }

                if let Some(angle_open) = link_buf.find('<') {
                    if let Some(angle_close) = link_buf.rfind('>') {
                        let label = link_buf[..angle_open].trim();
                        let url = link_buf[angle_open + 1..angle_close].trim();
                        inlines.push(Inline::Link {
                            content: vec![Inline::plain(label)],
                            target: LinkTarget::External(url.to_string()),
                        });
                        continue;
                    }
                }

                inlines.push(Inline::Link {
                    content: vec![Inline::plain(&link_buf)],
                    target: LinkTarget::Anchor(link_buf.clone()),
                });
                continue;
            }

            // 3. Bold **bold**
            if c == '*' && chars.peek() == Some(&'*') {
                chars.next(); // consume second *
                if !buf.is_empty() {
                    inlines.push(Inline::plain(std::mem::take(&mut buf)));
                }
                let mut bold_buf = String::new();
                while let Some(nc) = chars.next() {
                    if nc == '*' && chars.peek() == Some(&'*') {
                        chars.next();
                        break;
                    }
                    bold_buf.push(nc);
                }
                inlines.push(Inline::Text {
                    text: bold_buf,
                    style: Style {
                        bold: true,
                        italic: false,
                        strike: false,
                        code: false,
                    },
                });
                continue;
            }

            // 4. Italic *italic*
            if c == '*' {
                if !buf.is_empty() {
                    inlines.push(Inline::plain(std::mem::take(&mut buf)));
                }
                let mut italic_buf = String::new();
                while let Some(nc) = chars.next() {
                    if nc == '*' {
                        break;
                    }
                    italic_buf.push(nc);
                }
                inlines.push(Inline::Text {
                    text: italic_buf,
                    style: Style {
                        bold: false,
                        italic: true,
                        strike: false,
                        code: false,
                    },
                });
                continue;
            }

            // 5. Substitutions |name|
            if c == '|' {
                let mut sub_name = String::new();
                let mut found_end = false;
                while let Some(&nc) = chars.peek() {
                    if nc == '|' {
                        chars.next();
                        found_end = true;
                        break;
                    }
                    sub_name.push(chars.next().unwrap());
                }
                if found_end {
                    if !buf.is_empty() {
                        inlines.push(Inline::plain(std::mem::take(&mut buf)));
                    }
                    let replacement = self
                        .substitutions
                        .get(&sub_name)
                        .cloned()
                        .unwrap_or_else(|| format!("|{sub_name}|"));
                    inlines.push(Inline::plain(replacement));
                    continue;
                } else {
                    buf.push('|');
                    buf.push_str(&sub_name);
                    continue;
                }
            }

            // 6. Roles :role:`content` or Footnote/Citation refs [#f1]_
            if c == ':' {
                let mut role_name = String::new();
                let mut is_role = false;
                while let Some(&nc) = chars.peek() {
                    if nc == ':' {
                        chars.next();
                        is_role = true;
                        break;
                    }
                    if nc.is_whitespace() || nc == '`' {
                        break;
                    }
                    role_name.push(chars.next().unwrap());
                }
                if is_role && chars.peek() == Some(&'`') {
                    chars.next(); // consume `
                    if !buf.is_empty() {
                        inlines.push(Inline::plain(std::mem::take(&mut buf)));
                    }
                    let mut role_val = String::new();
                    while let Some(nc) = chars.next() {
                        if nc == '`' {
                            break;
                        }
                        role_val.push(nc);
                    }
                    match role_name.as_str() {
                        "strong" => inlines.push(Inline::Text {
                            text: role_val,
                            style: Style {
                                bold: true,
                                italic: false,
                                strike: false,
                                code: false,
                            },
                        }),
                        "emphasis" => inlines.push(Inline::Text {
                            text: role_val,
                            style: Style {
                                bold: false,
                                italic: true,
                                strike: false,
                                code: false,
                            },
                        }),
                        "literal" => inlines.push(Inline::Text {
                            text: role_val,
                            style: Style {
                                bold: false,
                                italic: false,
                                strike: false,
                                code: true,
                            },
                        }),
                        "ref" => inlines.push(Inline::Link {
                            content: vec![Inline::plain(&role_val)],
                            target: LinkTarget::Anchor(role_val.clone()),
                        }),
                        _ => inlines.push(Inline::plain(role_val)),
                    }
                    continue;
                } else {
                    buf.push(':');
                    buf.push_str(&role_name);
                    if is_role {
                        buf.push(':');
                    }
                    continue;
                }
            }

            // 7. Footnote / Citation Ref [#f1]_ or [CIT2024]_
            if c == '[' {
                let mut ref_name = String::new();
                let mut is_ref = false;
                while let Some(&nc) = chars.peek() {
                    if nc == ']' {
                        chars.next();
                        is_ref = true;
                        break;
                    }
                    ref_name.push(chars.next().unwrap());
                }
                if is_ref && chars.peek() == Some(&'_') {
                    chars.next(); // consume _
                    if !buf.is_empty() {
                        inlines.push(Inline::plain(std::mem::take(&mut buf)));
                    }
                    let clean_id = ref_name.trim_start_matches('#').to_string();
                    inlines.push(Inline::NoteRef(clean_id));
                    continue;
                } else {
                    buf.push('[');
                    buf.push_str(&ref_name);
                    if is_ref {
                        buf.push(']');
                    }
                    continue;
                }
            }

            // 8. Plain character
            buf.push(c);
        }

        if !buf.is_empty() {
            inlines.push(Inline::plain(buf));
        }

        inlines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rst_basic_heading_and_paragraph() {
        let rst = "=====\nTitle\n=====\n\nHello world\n";
        let doc = parse(rst.as_bytes()).unwrap();
        assert_eq!(doc.blocks.len(), 2);
        match &doc.blocks[0] {
            Block::Heading { level, content, .. } => {
                assert_eq!(*level, 1);
                assert_eq!(content.len(), 1);
            }
            _ => panic!("Expected Heading"),
        }
    }
}
