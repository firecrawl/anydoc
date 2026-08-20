use super::document_to_markdown;
use crate::model::{
    AnchorId, Block, Cell, Document, GridBuilder, ImageSource, Inline, LinkTarget, List, ListItem,
    MarkerKind, Note, NoteKind, Style, Table, TableKind,
};

fn doc(blocks: Vec<Block>) -> String {
    document_to_markdown(&Document { blocks, notes: Vec::new(), assets: Vec::new() })
}

fn styled(text: &str, style: Style) -> Inline {
    Inline::Text { text: text.into(), style }
}

fn heading(level: u8, text: &str) -> Block {
    Block::heading(level, vec![Inline::plain(text)])
}

fn table_from(rows: Vec<Vec<Cell>>, header_rows: usize) -> Block {
    Block::Table(Table::from_rows(rows, header_rows, TableKind::Data))
}

const BOLD: Style = Style { bold: true, italic: false, strike: false, code: false };
const ITALIC: Style = Style { bold: false, italic: true, strike: false, code: false };

#[test]
fn heading_and_paragraph() {
    let md = doc(vec![heading(2, "Title"), Block::Paragraph(vec![Inline::plain("Hello world.")])]);
    assert_eq!(md, "## Title\n\nHello world.\n");
}

#[test]
fn math_renders_in_dollar_delimiters_and_text_dollars_are_escaped() {
    let md = doc(vec![
        Block::Paragraph(vec![
            Inline::plain("Costs $5 or $6, and "),
            Inline::Math("x_1 < y".into()),
            Inline::plain(" holds."),
        ]),
        Block::Math("\\sum_{i=1}^{n} i $".into()),
    ]);
    assert_eq!(md, "Costs \\$5 or \\$6, and $x_1 < y$ holds.\n\n$$\n\\sum_{i=1}^{n} i \\$\n$$\n");
}

#[test]
fn math_in_a_table_cell_escapes_pipes() {
    let cell = |inlines| Cell { blocks: vec![Block::Paragraph(inlines)], col_span: 1, row_span: 1 };
    let md = doc(vec![table_from(
        vec![vec![cell(vec![Inline::plain("abs")]), cell(vec![Inline::Math("|x|".into())])]],
        0,
    )]);
    assert!(md.contains("| $\\|x\\|$ |"), "{md}");
}

#[test]
fn escapes_paired_syntax_chars() {
    let md = doc(vec![Block::Paragraph(vec![Inline::plain("a *bold* _it_ ~st~ `code`")])]);
    assert_eq!(md, "a \\*bold* \\_it_ \\~st~ \\`code`\n");
    let md = doc(vec![Block::Paragraph(vec![Inline::plain("see [really] and <b>hi</b>")])]);
    assert_eq!(md, "see \\[really] and \\<b>hi\\</b>\n");
}

#[test]
fn lone_syntax_chars_left_alone() {
    let md = doc(vec![Block::Paragraph(vec![Inline::plain("2 * 3 = 6 and 5*6 #tag")])]);
    assert_eq!(md, "2 * 3 = 6 and 5*6 #tag\n");
    let md = doc(vec![Block::Paragraph(vec![Inline::plain("x < 5, ~10%, file_name, a[1")])]);
    assert_eq!(md, "x < 5, ~10%, file_name, a[1\n");
}

#[test]
fn partners_that_cannot_close_leave_delimiters_raw() {
    // The space-padded `*` is not right-flanking, so the opener is inert.
    let md = doc(vec![Block::Paragraph(vec![Inline::plain("a *b 2 * 3")])]);
    assert_eq!(md, "a *b 2 * 3\n");
    // Same across a run seam.
    let md = doc(vec![Block::Paragraph(vec![
        Inline::plain("a *"),
        Inline::LineBreak,
        Inline::plain("2 * 3"),
    ])]);
    assert_eq!(md, "a *\\\n2 * 3\n");
    // Intraword underscores can neither open nor close.
    let md = doc(vec![Block::Paragraph(vec![
        Inline::plain("a _"),
        Inline::LineBreak,
        Inline::plain("snake_case"),
    ])]);
    assert_eq!(md, "a _\\\nsnake_case\n");
    // A `*` after punctuation and before a word character is not
    // right-flanking either.
    let md = doc(vec![Block::Paragraph(vec![Inline::plain("a *b .*c")])]);
    assert_eq!(md, "a *b .*c\n");
    // Flanking is judged at the delimiter run's edges: `__` between
    // letters is intraword even though each `_` neighbours the other.
    let md = doc(vec![Block::Paragraph(vec![Inline::plain("a _x foo__bar")])]);
    assert_eq!(md, "a _x foo__bar\n");
}

#[test]
fn intraword_underscores_unescaped() {
    let md = doc(vec![Block::Paragraph(vec![Inline::plain("snake_case_name vs _lead_")])]);
    assert_eq!(md, "snake_case_name vs \\_lead_\n");
}

#[test]
fn escapes_line_start_only_chars() {
    let md = doc(vec![Block::Paragraph(vec![Inline::plain("- not a list")])]);
    assert_eq!(md, "\\- not a list\n");
    let md = doc(vec![Block::Paragraph(vec![Inline::plain("1. not a list")])]);
    assert_eq!(md, "1\\. not a list\n");
    let md = doc(vec![Block::Paragraph(vec![Inline::plain("take 2. then rest")])]);
    assert_eq!(md, "take 2. then rest\n");
}

#[test]
fn line_start_lookalikes_unescaped() {
    let md = doc(vec![Block::Paragraph(vec![Inline::plain("-5°C at dawn")])]);
    assert_eq!(md, "-5°C at dawn\n");
    let md = doc(vec![Block::Paragraph(vec![Inline::plain("1.5 million users")])]);
    assert_eq!(md, "1.5 million users\n");
    let md = doc(vec![Block::Paragraph(vec![Inline::plain("#hashtag first")])]);
    assert_eq!(md, "#hashtag first\n");
    let md = doc(vec![Block::Paragraph(vec![Inline::plain("--- ruled")])]);
    assert_eq!(md, "--- ruled\n");
    let md = doc(vec![Block::Paragraph(vec![Inline::plain("---")])]);
    assert_eq!(md, "\\---\n");
}

#[test]
fn negative_number_in_cell_unescaped() {
    let md = doc(vec![table_from(
        vec![vec![
            Cell::from_inlines(vec![Inline::plain("-42")]),
            Cell::from_inlines(vec![Inline::plain("x")]),
        ]],
        0,
    )]);
    assert_eq!(md, "|  |  |\n| --- | --- |\n| -42 | x |\n");
}

#[test]
fn trailing_delimiter_before_styled_run_escaped() {
    let md = doc(vec![Block::Paragraph(vec![Inline::plain("star*"), styled("x", BOLD)])]);
    assert_eq!(md, "star\\***x**\n");
}

#[test]
fn delimiters_do_not_pair_across_run_seams() {
    // Two lines each ending in a backtick must not form a code span (#45).
    let md = doc(vec![Block::Paragraph(vec![
        Inline::plain("a `"),
        Inline::LineBreak,
        Inline::plain("b `"),
    ])]);
    assert_eq!(md, "a \\`\\\nb `\n");
    // Emphasis pairs across a hard break just as code spans do.
    let md = doc(vec![Block::Paragraph(vec![
        Inline::plain("a *"),
        Inline::LineBreak,
        Inline::plain("b*"),
    ])]);
    assert_eq!(md, "a \\*\\\nb*\n");
    // A raw backtick pairs with a later code span's fence.
    let md = doc(vec![Block::Paragraph(vec![
        Inline::plain("a `"),
        Inline::LineBreak,
        styled("x", Style { code: true, ..Style::PLAIN }),
    ])]);
    assert_eq!(md, "a \\`\\\n`x`\n");
}

#[test]
fn unresolved_link_fallback_counts_toward_seam_pairing() {
    // The fallback text supplies the `]` that pairs with the earlier `[`.
    let md = doc(vec![Block::Paragraph(vec![
        Inline::plain("[click"),
        Inline::LineBreak,
        Inline::Link {
            content: vec![Inline::plain("here]")],
            target: LinkTarget::Anchor("nowhere".into()),
        },
        Inline::plain("(https://e.test)"),
    ])]);
    assert_eq!(md, "\\[click\\\nhere](https://e.test)\n");
    // Same seam with an emphasis delimiter in the fallback.
    let md = doc(vec![Block::Paragraph(vec![
        Inline::plain("a *"),
        Inline::LineBreak,
        Inline::Link {
            content: vec![Inline::plain("b*")],
            target: LinkTarget::Anchor("nowhere".into()),
        },
    ])]);
    assert_eq!(md, "a \\*\\\nb*\n");
}

#[test]
fn escaped_backtick_in_later_run_still_pairs() {
    // A styled run's backtick is emitted as `\\``, yet still closes a span
    // an earlier raw backtick opens: code spans ignore backslash escapes.
    let md = doc(vec![Block::Paragraph(vec![
        Inline::plain("a `"),
        Inline::LineBreak,
        styled("x`y", BOLD),
    ])]);
    assert_eq!(md, "a \\`\\\n**x\\`y**\n");
    // A whitespace-only code run loses its styling and emits no fence.
    let md = doc(vec![Block::Paragraph(vec![
        Inline::plain("a `"),
        Inline::LineBreak,
        Inline::Link {
            content: vec![styled("  ", Style { code: true, ..Style::PLAIN }), Inline::plain("x")],
            target: LinkTarget::External("https://e.test".into()),
        },
    ])]);
    assert_eq!(md, "a `\\\n[  x](https://e.test)\n");
}

#[test]
fn bold_trailing_space_moved_out() {
    let md = doc(vec![Block::Paragraph(vec![styled("bold ", BOLD), Inline::plain("plain")])]);
    assert_eq!(md, "**bold** plain\n");
}

#[test]
fn adjacent_same_style_runs_merged() {
    let md = doc(vec![Block::Paragraph(vec![styled("bo", BOLD), styled("ld", BOLD)])]);
    assert_eq!(md, "**bold**\n");
}

#[test]
fn bold_italic_combo() {
    let md = doc(vec![Block::Paragraph(vec![styled(
        "both",
        Style { bold: true, italic: true, strike: false, code: false },
    )])]);
    assert_eq!(md, "***both***\n");
}

#[test]
fn whitespace_only_run_loses_styling() {
    let md = doc(vec![Block::Paragraph(vec![
        styled("a", BOLD),
        styled(" ", ITALIC),
        styled("b", BOLD),
    ])]);
    assert_eq!(md, "**a b**\n");
}

#[test]
fn link_render() {
    let md = doc(vec![Block::Paragraph(vec![Inline::Link {
        content: vec![Inline::plain("site")],
        target: LinkTarget::External("https://example.com/a(b)".into()),
    }])]);
    assert_eq!(md, "[site](<https://example.com/a(b)>)\n");
}

#[test]
fn relative_links_preserved() {
    let md = doc(vec![Block::Paragraph(vec![Inline::Link {
        content: vec![Inline::plain("next")],
        target: LinkTarget::Relative("chapter2.xhtml".into()),
    }])]);
    assert_eq!(md, "[next](chapter2.xhtml)\n");
    let md = doc(vec![Block::Paragraph(vec![Inline::Link {
        content: vec![Inline::plain("mail")],
        target: LinkTarget::External("mailto:a@b.c".into()),
    }])]);
    assert_eq!(md, "[mail](mailto:a@b.c)\n");
}

#[test]
fn unresolved_anchor_degrades_to_text() {
    let md = doc(vec![Block::Paragraph(vec![Inline::Link {
        content: vec![Inline::plain("note")],
        target: LinkTarget::Anchor("nowhere".into()),
    }])]);
    assert_eq!(md, "note\n");
}

#[test]
fn sourceless_image_renders_alt_text() {
    let md = doc(vec![Block::Paragraph(vec![Inline::Image {
        alt: "chart".into(),
        source: ImageSource::Unavailable,
    }])]);
    assert_eq!(md, "chart\n");
}

#[test]
fn composite_marker_labels_are_escaped() {
    // M15: source-derived labels must not alter Markdown structure.
    let list = crate::model::List {
        marker: crate::model::MarkerKind::Decimal,
        start: 1,
        items: vec![crate::model::ListItem {
            blocks: vec![Block::Paragraph(vec![Inline::plain("x")])],
            checked: None,
            marker_label: Some("#1\n*a*".into()),
        }],
    };
    let md = doc(vec![Block::List(list)]);
    assert_eq!(md.lines().count(), 1, "control characters must not split the marker");
    assert!(!md.contains("*a*"), "emphasis in a label must be escaped: {md}");
    assert!(md.starts_with("- "), "labelled items render as literal markers: {md}");
}

#[test]
fn layout_table_unwrapped() {
    let mut b = GridBuilder::new();
    b.next_row();
    b.place(Cell::new(vec![heading(1, "Boxed"), Block::Paragraph(vec![Inline::plain("body")])]))
        .unwrap();
    let md = doc(vec![Block::Table(b.finish(TableKind::Layout))]);
    assert_eq!(md, "# Boxed\n\nbody\n");
}

#[test]
fn data_table_1x1_not_unwrapped() {
    let md = doc(vec![table_from(vec![vec![Cell::from_inlines(vec![Inline::plain("x")])]], 0)]);
    assert_eq!(md, "|  |\n| --- |\n| x |\n");
}

#[test]
fn trailing_empty_rows_and_columns_trimmed() {
    let empty = Cell::default;
    let filled = |s: &str| Cell::from_inlines(vec![Inline::plain(s)]);
    let md = doc(vec![table_from(
        vec![
            vec![filled("a"), filled("b"), empty()],
            vec![filled("c"), empty(), empty()],
            vec![empty(), empty(), empty()],
        ],
        1,
    )]);
    assert_eq!(md, "| a | b |\n| --- | --- |\n| c |  |\n");
}

#[test]
fn table_basic() {
    let md = doc(vec![table_from(
        vec![
            vec![
                Cell::from_inlines(vec![Inline::plain("Name")]),
                Cell::from_inlines(vec![Inline::plain("Age")]),
            ],
            vec![
                Cell::from_inlines(vec![Inline::plain("Ann | Bob")]),
                Cell::from_inlines(vec![Inline::plain("30")]),
            ],
        ],
        1,
    )]);
    assert_eq!(md, "| Name | Age |\n| --- | --- |\n| Ann \\| Bob | 30 |\n");
}

#[test]
fn table_headerless_and_ragged() {
    let md = doc(vec![table_from(
        vec![
            vec![Cell::from_inlines(vec![Inline::plain("a")])],
            vec![
                Cell::from_inlines(vec![Inline::plain("b")]),
                Cell::from_inlines(vec![Inline::plain("c")]),
            ],
        ],
        0,
    )]);
    assert_eq!(md, "|  |  |\n| --- | --- |\n| a |  |\n| b | c |\n");
}

#[test]
fn table_multiparagraph_cell() {
    let cell = Cell::new(vec![
        Block::Paragraph(vec![Inline::plain("one")]),
        Block::Paragraph(vec![Inline::plain("two")]),
    ]);
    let md =
        doc(vec![table_from(vec![vec![cell, Cell::from_inlines(vec![Inline::plain("x")])]], 0)]);
    assert_eq!(md, "|  |  |\n| --- | --- |\n| one<br>two | x |\n");
}

#[test]
fn merged_cells_render_blank_covered_positions() {
    let mut b = GridBuilder::new();
    b.next_row();
    b.place(Cell::spanning(vec![Block::Paragraph(vec![Inline::plain("wide")])], 2, 1)).unwrap();
    b.place(Cell::from_inlines(vec![Inline::plain("end")])).unwrap();
    b.next_row();
    for t in ["a", "b", "c"] {
        b.place(Cell::from_inlines(vec![Inline::plain(t)])).unwrap();
    }
    let mut table = b.finish(TableKind::Data);
    table.header_rows = 1;
    let md = doc(vec![Block::Table(table)]);
    assert_eq!(md, "| wide |  | end |\n| --- | --- | --- |\n| a | b | c |\n");
}

#[test]
fn trailing_covered_columns_are_preserved() {
    let mut b = GridBuilder::new();
    b.next_row();
    b.place(Cell::spanning(vec![Block::Paragraph(vec![Inline::plain("wide")])], 3, 1)).unwrap();
    let mut table = b.finish(TableKind::Data);
    table.header_rows = 1;
    let md = doc(vec![Block::Table(table)]);
    assert_eq!(md, "| wide |  |  |\n| --- | --- | --- |\n");
}

#[test]
fn url_pipes_cannot_split_table_cells() {
    let cell = Cell::from_inlines(vec![Inline::Link {
        content: Vec::new(),
        target: LinkTarget::External("https://e.test/a|b".into()),
    }]);
    let md = doc(vec![table_from(vec![vec![cell]], 0)]);
    assert_eq!(md, "|  |\n| --- |\n| [https://e.test/a\\|b](https://e.test/a%7Cb) |\n");
}

#[test]
fn code_span_pipes_cannot_split_table_cells() {
    let code = |t: &str| {
        Cell::from_inlines(vec![Inline::Text {
            text: t.into(),
            style: Style { code: true, ..Style::PLAIN },
        }])
    };
    let md = doc(vec![table_from(vec![vec![code("a | b"), code(r"a \| b")]], 0)]);
    assert_eq!(md, concat!("|  |  |\n", "| --- | --- |\n", r"| `a \| b` | `a \\\| b` |", "\n"));
}

#[test]
fn url_angle_brackets_are_encoded_without_bracketing() {
    let md = doc(vec![Block::Paragraph(vec![Inline::Link {
        content: vec![Inline::plain("link")],
        target: LinkTarget::External("https://e.test/a<b>c".into()),
    }])]);
    assert_eq!(
        md,
        "[link](https://e.test/a%3Cb%3Ec)
"
    );
}

#[test]
fn url_controls_cannot_split_the_document() {
    let md = doc(vec![Block::Paragraph(vec![Inline::Link {
        content: vec![Inline::plain("link")],
        target: LinkTarget::External("https://e.test/a\nb".into()),
    }])]);
    assert_eq!(md, "[link](https://e.test/a%0Ab)\n");
}

#[test]
fn nested_list() {
    let md = doc(vec![Block::List(List {
        marker: MarkerKind::Bullet,
        start: 1,
        items: vec![ListItem {
            blocks: vec![
                Block::Paragraph(vec![Inline::plain("outer")]),
                Block::List(List {
                    marker: MarkerKind::Decimal,
                    start: 3,
                    items: vec![ListItem {
                        blocks: vec![Block::Paragraph(vec![Inline::plain("inner")])],
                        checked: None,
                        marker_label: None,
                    }],
                }),
            ],
            checked: None,
            marker_label: None,
        }],
    })]);
    assert_eq!(md, "- outer\n\n  3. inner\n");
}

#[test]
fn roman_and_alpha_markers_render_literally() {
    let item = |text: &str| ListItem {
        blocks: vec![Block::Paragraph(vec![Inline::plain(text)])],
        checked: None,
        marker_label: None,
    };
    let md = doc(vec![
        Block::List(List {
            marker: MarkerKind::LowerRoman,
            start: 3,
            items: vec![item("third"), item("fourth")],
        }),
        Block::List(List {
            marker: MarkerKind::UpperAlpha,
            start: 27,
            items: vec![item("double letters")],
        }),
    ]);
    assert_eq!(md, "- iii. third\n- iv. fourth\n\n- AA. double letters\n");
}

#[test]
fn code_span_with_backticks() {
    let md = doc(vec![Block::Paragraph(vec![styled("a`b", Style { code: true, ..Style::PLAIN })])]);
    assert_eq!(md, "``a`b``\n");
}

#[test]
fn hard_break() {
    let md = doc(vec![Block::Paragraph(vec![
        Inline::plain("line one"),
        Inline::LineBreak,
        Inline::plain("line two"),
    ])]);
    assert_eq!(md, "line one\\\nline two\n");
}

#[test]
fn line_start_escape_after_hard_break() {
    let md = doc(vec![Block::Paragraph(vec![
        Inline::plain("intro"),
        Inline::LineBreak,
        Inline::plain("- dash"),
    ])]);
    assert_eq!(md, "intro\\\n\\- dash\n");
}

#[test]
fn blockquote() {
    let md = doc(vec![Block::BlockQuote(vec![Block::Paragraph(vec![Inline::plain("quoted")])])]);
    assert_eq!(md, "> quoted\n");
}

#[test]
fn empty_paragraphs_dropped() {
    let md = doc(vec![
        Block::Paragraph(vec![Inline::plain("  ")]),
        Block::Paragraph(vec![]),
        Block::Paragraph(vec![Inline::plain("real")]),
    ]);
    assert_eq!(md, "real\n");
}

#[test]
fn entity_escaped_but_plain_ampersand_kept() {
    let md = doc(vec![Block::Paragraph(vec![Inline::plain("A & B &amp; C")])]);
    assert_eq!(md, "A & B &amp;amp; C\n");
}

fn note(id: &str, blocks: Vec<Block>) -> Note {
    Note { id: id.into(), kind: NoteKind::Footnote, blocks }
}

#[test]
fn footnotes() {
    let md = document_to_markdown(&Document {
        blocks: vec![Block::Paragraph(vec![
            Inline::plain("Claim."),
            Inline::NoteRef("b".into()),
            Inline::plain(" More."),
            Inline::NoteRef("a".into()),
        ])],
        notes: vec![
            note("a", vec![Block::Paragraph(vec![Inline::plain("Second note.")])]),
            note(
                "b",
                vec![
                    Block::Paragraph(vec![Inline::plain("First note.")]),
                    Block::Paragraph(vec![Inline::plain("With a second paragraph.")]),
                ],
            ),
        ],
        assets: Vec::new(),
    });
    assert_eq!(
        md,
        "Claim.[^1] More.[^2]\n\n[^1]: First note.\n\n    With a second paragraph.\n\n[^2]: Second note.\n"
    );
}

#[test]
fn empty_and_unreferenced_notes() {
    let md = document_to_markdown(&Document {
        blocks: vec![Block::Paragraph(vec![
            Inline::plain("Text"),
            Inline::NoteRef("empty".into()),
        ])],
        notes: vec![
            note("empty", vec![Block::Paragraph(vec![])]),
            note("orphan", vec![Block::Paragraph(vec![Inline::plain("Kept.")])]),
        ],
        assets: Vec::new(),
    });
    assert_eq!(md, "Text\n\n[^1]: Kept.\n");
}

#[test]
fn duplicate_note_ids_render_one_definition() {
    let md = document_to_markdown(&Document {
        blocks: vec![Block::Paragraph(vec![Inline::plain("Text"), Inline::NoteRef("a".into())])],
        notes: vec![
            note("a", vec![Block::Paragraph(vec![Inline::plain("First wins.")])]),
            note("a", vec![Block::Paragraph(vec![Inline::plain("Duplicate dropped.")])]),
        ],
        assets: Vec::new(),
    });
    assert_eq!(md, "Text[^1]\n\n[^1]: First wins.\n");
}

#[test]
fn blank_duplicate_note_does_not_suppress_a_later_definition() {
    let md = document_to_markdown(&Document {
        blocks: vec![Block::Paragraph(vec![Inline::plain("Text"), Inline::NoteRef("a".into())])],
        notes: vec![
            note("a", vec![Block::Paragraph(Vec::new())]),
            note("a", vec![Block::Paragraph(vec![Inline::plain("Usable.")])]),
        ],
        assets: Vec::new(),
    });
    assert_eq!(md, "Text[^1]\n\n[^1]: Usable.\n");
}

#[test]
fn an_empty_item_keeps_its_marker_and_the_numbering() {
    let item = |text: &str| ListItem {
        blocks: if text.is_empty() {
            Vec::new()
        } else {
            vec![Block::Paragraph(vec![Inline::plain(text)])]
        },
        checked: None,
        marker_label: None,
    };
    let md = doc(vec![Block::List(List {
        marker: MarkerKind::Decimal,
        start: 1,
        items: vec![item("one"), item(""), item("three")],
    })]);
    assert_eq!(
        md,
        "1. one
2. 
3. three
"
    );
}

#[test]
fn task_list() {
    let md = doc(vec![Block::List(List {
        marker: MarkerKind::Bullet,
        start: 1,
        items: vec![
            ListItem {
                blocks: vec![Block::Paragraph(vec![Inline::plain("done")])],
                checked: Some(true),
                marker_label: None,
            },
            ListItem {
                blocks: vec![Block::Paragraph(vec![Inline::plain("todo")])],
                checked: Some(false),
                marker_label: None,
            },
        ],
    })]);
    assert_eq!(md, "- [x] done\n- [ ] todo\n");
}

// ---------------------------------------------------------------------------
// New contexts: link labels, image alt, anchors

#[test]
fn link_label_bracket_escaped() {
    let md = doc(vec![Block::Paragraph(vec![Inline::Link {
        content: vec![Inline::plain("x]")],
        target: LinkTarget::External("https://e.com".into()),
    }])]);
    assert_eq!(md, "[x\\]](https://e.com)\n");
}

#[test]
fn image_alt_brackets_and_backslash_escaped() {
    let md = doc(vec![Block::Paragraph(vec![Inline::Image {
        alt: "a[b]c\\".into(),
        source: ImageSource::External("https://e.com/i.png".into()),
    }])]);
    assert_eq!(md, "![a\\[b\\]c\\\\](https://e.com/i.png)\n");
}

#[test]
fn anchor_on_plain_paragraph_round_trips() {
    let mark: AnchorId = "My Mark".into();
    let md = doc(vec![
        Block::Paragraph(vec![Inline::Anchor(mark.clone()), Inline::plain("Target here.")]),
        Block::Paragraph(vec![Inline::Link {
            content: vec![Inline::plain("jump")],
            target: LinkTarget::Anchor(mark),
        }]),
    ]);
    assert_eq!(md, "<a id=\"my-mark\"></a>Target here.\n\n[jump](#my-mark)\n");
}

#[test]
fn unreferenced_anchor_renders_nothing() {
    // An anchor no link targets is unreachable: producers mark up far more
    // positions than they reference.
    let md = doc(vec![Block::Paragraph(vec![
        Inline::Anchor("standalone-mark".into()),
        Inline::plain("No link points here."),
    ])]);
    assert_eq!(md, "No link points here.\n");
}

#[test]
fn heading_coincident_anchor_uses_slug() {
    let md = doc(vec![
        Block::Heading {
            level: 2,
            anchor: Some("bm1".into()),
            content: vec![Inline::plain("Section Two")],
        },
        Block::Paragraph(vec![Inline::Link {
            content: vec![Inline::plain("go")],
            target: LinkTarget::Anchor("bm1".into()),
        }]),
    ]);
    assert_eq!(md, "## Section Two\n\n[go](#section-two)\n");
}

#[test]
fn duplicate_heading_slugs_deduped() {
    let md = doc(vec![
        heading(1, "Same"),
        Block::Heading { level: 1, anchor: Some("x".into()), content: vec![Inline::plain("Same")] },
        Block::Paragraph(vec![Inline::Link {
            content: vec![Inline::plain("second")],
            target: LinkTarget::Anchor("x".into()),
        }]),
    ]);
    assert_eq!(md, "# Same\n\n# Same\n\n[second](#same-1)\n");
}

#[test]
fn code_block_in_cell_uses_code_span() {
    let cell = Cell::new(vec![Block::CodeBlock { lang: None, text: "let `x` = 1;".into() }]);
    let md = doc(vec![table_from(vec![vec![cell]], 0)]);
    assert_eq!(md, "|  |\n| --- |\n| ``let `x` = 1;`` |\n");
}

#[test]
fn code_block_keeps_its_language_hint() {
    let md = doc(vec![Block::CodeBlock { lang: Some("rust".into()), text: "fn main() {}".into() }]);
    assert_eq!(md, "```rust\nfn main() {}\n```\n");
}

#[test]
fn cell_edge_whitespace_is_trimmed() {
    // S10: cell padding from spreadsheets and CSV is layout, and the table's
    // own padding would swallow it anyway.
    let md = doc(vec![table_from(
        vec![vec![
            Cell::from_inlines(vec![Inline::plain("  padded\t")]),
            Cell::from_inlines(vec![Inline::plain("plain")]),
        ]],
        0,
    )]);
    assert_eq!(md, "|  |  |\n| --- | --- |\n| padded | plain |\n");
}
