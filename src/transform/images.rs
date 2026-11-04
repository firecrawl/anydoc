//! Fork (sznb): image extraction for mixed text/image output.
//!
//! [`extract_images`] walks a parsed [`Document`], measures every embedded
//! image (header-only, see `shared::image_meta`), drops images smaller than
//! the threshold, and replaces the rest in place by a placeholder token
//! (`[IMG::<name>]` by default). The extracted payloads come back in an
//! [`ImageReport`] so a caller can store them (e.g. upload to object storage)
//! under the very names the placeholders carry. Everything stays in memory.
//!
//! Names are content-addressed (`{prefix}{sha256[..16]}{ext}`): the same
//! picture referenced twice, or embedded twice under different parts, maps to
//! one image and one name, and re-uploading it is idempotent.

use std::collections::HashMap;

use sha2::{Digest, Sha256};

use crate::model::{AssetId, Block, CellSlot, Document, ImageSource, Inline};
use crate::shared::image_meta;

/// What to extract and how to name it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageOptions {
    /// Images narrower than this many pixels are dropped (0 disables).
    pub min_width: u32,
    /// Images shorter than this many pixels are dropped (0 disables).
    pub min_height: u32,
    /// Placeholder template; `{name}` is replaced by the image's file name.
    pub placeholder: String,
    /// Prefix of every generated file name.
    pub name_prefix: String,
}

impl Default for ImageOptions {
    fn default() -> Self {
        ImageOptions {
            min_width: 50,
            min_height: 50,
            placeholder: "[IMG::{name}]".to_string(),
            name_prefix: "img_".to_string(),
        }
    }
}

/// One extracted image.
#[derive(Debug, Clone)]
pub struct ExtractedImage {
    /// Content-addressed file name, e.g. `img_3f2a9c0d1b2e4f56.png`.
    pub name: String,
    /// The token that stands in for it in the rendered output.
    pub placeholder: String,
    /// MIME type (sniffed from the bytes when recognizable).
    pub media_type: String,
    /// Pixel width, when the header gives one.
    pub width: Option<u32>,
    /// Pixel height, when the header gives one.
    pub height: Option<u32>,
    /// The payload, exactly as embedded.
    pub bytes: Vec<u8>,
    /// How many places in the document reference it.
    pub occurrences: usize,
    /// Package part or stream the first copy came from.
    pub origin_part: String,
}

/// The result of [`extract_images`].
#[derive(Debug, Clone, Default)]
pub struct ImageReport {
    /// Extracted images, in order of first appearance (body, then notes).
    pub images: Vec<ExtractedImage>,
    /// Image references dropped for being under the size threshold.
    pub skipped: usize,
}

#[derive(Clone, Copy)]
enum Decision {
    /// Keep: index into `ImageReport::images`.
    Keep(usize),
    /// Under the size threshold.
    Drop,
    /// Not extractable (asset missing / empty): leave the inline untouched.
    Leave,
}

struct Extractor<'a> {
    opts: &'a ImageOptions,
    assets: &'a [crate::model::Asset],
    decided: HashMap<AssetId, Decision>,
    by_name: HashMap<String, usize>,
    report: ImageReport,
}

/// Extract the embedded images of `doc` in place: small ones are removed,
/// the rest are replaced by [`ImageSource::Placeholder`] tokens. External and
/// unavailable images are left as they are.
pub fn extract_images(doc: &mut Document, opts: &ImageOptions) -> ImageReport {
    let assets = std::mem::take(&mut doc.assets);
    let mut ex = Extractor {
        opts,
        assets: &assets,
        decided: HashMap::new(),
        by_name: HashMap::new(),
        report: ImageReport::default(),
    };
    ex.blocks(&mut doc.blocks);
    for note in &mut doc.notes {
        ex.blocks(&mut note.blocks);
    }
    let report = ex.report;
    doc.assets = assets;
    report
}

impl Extractor<'_> {
    fn blocks(&mut self, blocks: &mut [Block]) {
        for block in blocks {
            match block {
                Block::Heading { content, .. } | Block::Paragraph(content) => self.inlines(content),
                Block::List(list) => {
                    for item in &mut list.items {
                        self.blocks(&mut item.blocks);
                    }
                }
                Block::Table(table) => {
                    for row in &mut table.grid {
                        for slot in row {
                            if let CellSlot::Origin(cell) = slot {
                                self.blocks(&mut cell.blocks);
                            }
                        }
                    }
                }
                Block::BlockQuote(inner) => self.blocks(inner),
                Block::CodeBlock { .. } | Block::Rule | Block::Math(_) => {}
            }
        }
    }

    fn inlines(&mut self, inlines: &mut Vec<Inline>) {
        inlines.retain_mut(|inline| match inline {
            Inline::Link { content, .. } => {
                self.inlines(content);
                true
            }
            Inline::Image { source, .. } => {
                let ImageSource::Asset(id) = *source else { return true };
                match self.decide(id) {
                    Decision::Keep(idx) => {
                        let image = &mut self.report.images[idx];
                        image.occurrences += 1;
                        *source = ImageSource::Placeholder(image.placeholder.clone());
                        true
                    }
                    Decision::Drop => {
                        self.report.skipped += 1;
                        false
                    }
                    Decision::Leave => true,
                }
            }
            _ => true,
        });
    }

    fn decide(&mut self, id: AssetId) -> Decision {
        if let Some(&decision) = self.decided.get(&id) {
            return decision;
        }
        let decision = self.classify(id);
        self.decided.insert(id, decision);
        decision
    }

    fn classify(&mut self, id: AssetId) -> Decision {
        let Some(asset) = self.assets.get(id.0) else { return Decision::Leave };
        if asset.bytes.is_empty() {
            return Decision::Leave;
        }
        // Only pictures: an embedded OLE payload (Word / PowerPoint object
        // shown through an image inline) keeps its "Embedded object" alt text,
        // as the python pipeline only ever extracts `media/image*` parts.
        if image_meta::sniff(&asset.bytes).is_none()
            && !asset.media_type.to_ascii_lowercase().starts_with("image/")
        {
            return Decision::Leave;
        }
        let dims = image_meta::dimensions(&asset.bytes);
        // Unknown size keeps the image (same rule as the python pipeline).
        if let Some((w, h)) = dims {
            if w < self.opts.min_width || h < self.opts.min_height {
                return Decision::Drop;
            }
        }
        let digest = Sha256::digest(&asset.bytes);
        let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
        let ext = image_meta::ext_for(&asset.bytes, &asset.media_type, &asset.origin_part);
        let name = format!("{}{}{}", self.opts.name_prefix, hex, ext);
        if let Some(&idx) = self.by_name.get(&name) {
            return Decision::Keep(idx);
        }
        let idx = self.report.images.len();
        self.report.images.push(ExtractedImage {
            placeholder: self.opts.placeholder.replace("{name}", &name),
            name: name.clone(),
            media_type: image_meta::media_type_for(&asset.bytes, &asset.media_type),
            width: dims.map(|d| d.0),
            height: dims.map(|d| d.1),
            bytes: asset.bytes.clone(),
            occurrences: 0,
            origin_part: asset.origin_part.clone(),
        });
        self.by_name.insert(name, idx);
        Decision::Keep(idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Asset, Cell, Table, TableKind};
    use crate::render::markdown::document_to_markdown_with;
    use crate::render::text::document_to_text_with;
    use crate::shared::image_meta::tests::png;

    fn asset(id: usize, bytes: Vec<u8>) -> Asset {
        Asset {
            id: AssetId(id),
            media_type: "image/png".into(),
            origin_part: format!("word/media/image{id}.png"),
            bytes,
        }
    }

    fn img(id: usize) -> Inline {
        Inline::Image { alt: "图".into(), source: ImageSource::Asset(AssetId(id)) }
    }

    fn sample() -> Document {
        let big = png(200, 100);
        let mut doc = Document::default();
        // 0: big; 1: 10x10 icon; 2: the big image again under another part.
        doc.assets = vec![asset(0, big.clone()), asset(1, png(10, 10)), asset(2, big)];
        doc.blocks = vec![
            Block::Paragraph(vec![Inline::plain("前文"), img(0), Inline::plain("后文")]),
            Block::Paragraph(vec![img(1)]),
            Block::Table(Table::from_rows(
                vec![vec![
                    Cell::from_inlines(vec![Inline::plain("单元格"), img(2)]),
                    Cell::from_inlines(vec![img(1), Inline::plain("x")]),
                ]],
                0,
                TableKind::Data,
            )),
        ];
        doc
    }

    #[test]
    fn extracts_filters_and_dedups() {
        let mut doc = sample();
        let report = extract_images(&mut doc, &ImageOptions::default());
        assert_eq!(report.images.len(), 1, "same bytes under two parts -> one image");
        assert_eq!(report.skipped, 2, "the 10x10 icon is referenced twice");
        let image = &report.images[0];
        assert_eq!(image.occurrences, 2);
        assert_eq!((image.width, image.height), (Some(200), Some(100)));
        assert!(image.name.starts_with("img_") && image.name.ends_with(".png"));
        assert_eq!(image.name.len(), "img_".len() + 16 + ".png".len());
        assert_eq!(image.placeholder, format!("[IMG::{}]", image.name));
        // Assets survive the walk.
        assert_eq!(doc.assets.len(), 3);

        let md = document_to_markdown_with(&doc, true, true);
        let token = &image.placeholder;
        assert!(md.contains(&format!("前文{token}后文")), "{md}");
        assert_eq!(md.matches(token.as_str()).count(), 2, "{md}");
        assert!(!md.contains('图'), "alt text is replaced, not appended: {md}");

        let gfm = document_to_markdown_with(&doc, false, true);
        assert_eq!(gfm.matches(token.as_str()).count(), 2, "{gfm}");

        let text = document_to_text_with(&doc, true);
        assert!(text.contains(&format!("前文{token}后文")), "{text}");
        assert!(text.contains(&format!("单元格{token}")), "{text}");
        // The image-only paragraph of the small icon disappears entirely.
        assert!(!text.contains("\n\n\n"), "{text:?}");
    }

    #[test]
    fn thresholds_and_template_are_configurable() {
        let mut doc = sample();
        let opts = ImageOptions {
            min_width: 0,
            min_height: 0,
            placeholder: "<<{name}>>".into(),
            name_prefix: "p_".into(),
        };
        let report = extract_images(&mut doc, &opts);
        assert_eq!(report.images.len(), 2);
        assert_eq!(report.skipped, 0);
        assert!(report.images.iter().all(|i| i.name.starts_with("p_")));
        assert_eq!(report.images[1].placeholder, format!("<<{}>>", report.images[1].name));
    }

    #[test]
    fn unknown_size_is_kept_and_external_untouched() {
        let mut doc = Document::default();
        doc.assets = vec![Asset {
            id: AssetId(0),
            media_type: "image/x-wmf".into(),
            origin_part: "word/media/image1.wmf".into(),
            bytes: vec![1, 0, 9, 0, 0, 3, 0, 0], // bare WMF: no extent
        }];
        doc.blocks = vec![Block::Paragraph(vec![
            img(0),
            Inline::Image { alt: "外链".into(), source: ImageSource::External("https://e.com/a.png".into()) },
        ])];
        let report = extract_images(&mut doc, &ImageOptions::default());
        assert_eq!(report.images.len(), 1);
        assert_eq!(report.images[0].width, None);
        assert!(report.images[0].name.ends_with(".wmf"));
        let md = document_to_markdown_with(&doc, true, true);
        assert!(md.contains("![外链](https://e.com/a.png)"), "{md}");
    }

    #[test]
    fn ole_payloads_keep_their_alt_text() {
        let mut doc = Document::default();
        doc.assets = vec![Asset {
            id: AssetId(0),
            media_type: "application/vnd.ms-ole-object".into(),
            origin_part: "word/embeddings/oleObject1.bin".into(),
            bytes: vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1],
        }];
        doc.blocks = vec![Block::Paragraph(vec![Inline::Image {
            alt: "Embedded object: Word.Document.12".into(),
            source: ImageSource::Asset(AssetId(0)),
        }])];
        let report = extract_images(&mut doc, &ImageOptions::default());
        assert!(report.images.is_empty() && report.skipped == 0);
        let md = document_to_markdown_with(&doc, true, true);
        assert!(md.contains("Embedded object: Word.Document.12"), "{md}");
    }

    #[test]
    fn external_only_document_is_untouched() {
        let mut doc = Document::default();
        doc.blocks = vec![Block::Paragraph(vec![Inline::Image {
            alt: "外链".into(),
            source: ImageSource::External("https://e.com/a.png".into()),
        }])];
        let report = extract_images(&mut doc, &ImageOptions::default());
        assert!(report.images.is_empty());
        let md = document_to_markdown_with(&doc, true, true);
        assert!(md.contains("![外链](https://e.com/a.png)"), "{md}");
    }
}
