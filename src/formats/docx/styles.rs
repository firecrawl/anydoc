//! WordprocessingML style table.
//!
//! Bold/italic/strike are *toggle properties* (ECMA-376 §17.7.3): within the
//! style hierarchy a `true` specification toggles the inherited value and a
//! `false` specification leaves it unchanged, so the style layers contribute
//! a true-count *parity* XORed over the `docDefaults` base. Direct run
//! formatting is absolute on/off.

use crate::error::ConvertError;
use crate::model::Style;
use crate::package::xml::{Element, ns};
use crate::shared::blockstyle::{self, BlockStyle};
use crate::shared::chain::StyleChains;
use crate::shared::delta::StyleDelta;

/// Per-property parity of `true` toggle specifications in a style chain.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Toggles {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
}

impl Toggles {
    pub fn xor(self, other: Toggles) -> Toggles {
        Toggles {
            bold: self.bold ^ other.bold,
            italic: self.italic ^ other.italic,
            strike: self.strike ^ other.strike,
        }
    }

    pub fn apply_over(self, base: Style) -> Style {
        Style {
            bold: base.bold ^ self.bold,
            italic: base.italic ^ self.italic,
            strike: base.strike ^ self.strike,
            code: base.code,
        }
    }
}

pub struct Styles<'a> {
    chains: StyleChains<'a, Element>,
    /// docDefaults as absolute values (the base the toggles flip over).
    pub doc_defaults: Style,
}

impl<'a> Styles<'a> {
    pub fn parse_opt(root: Option<&'a Element>) -> Styles<'a> {
        match root {
            Some(root) => Styles::parse(root),
            None => Styles { chains: StyleChains::default(), doc_defaults: Style::PLAIN },
        }
    }

    pub fn parse(root: &'a Element) -> Styles<'a> {
        let mut chains = StyleChains::default();
        for style in root.find_all(ns::W, "style") {
            if let Some(id) = style.attr(ns::W, "styleId") {
                let parent = style.find(ns::W, "basedOn").and_then(|e| e.attr(ns::W, "val"));
                chains.insert(id, style, parent);
            }
        }
        let doc_defaults = root
            .find(ns::W, "docDefaults")
            .and_then(|d| d.find(ns::W, "rPrDefault"))
            .and_then(|d| d.find(ns::W, "rPr"))
            .map(|rpr| rpr_delta(rpr).resolve())
            .unwrap_or(Style::PLAIN);
        Styles { chains, doc_defaults }
    }

    /// The parity of `true` toggle specifications along a style's `basedOn`
    /// chain. A `false` in a style leaves the inherited value unchanged.
    pub fn run_toggles(&self, id: &str) -> Result<Toggles, ConvertError> {
        let mut parity = Toggles::default();
        self.chains.walk::<()>(id, |style| {
            if let Some(rpr) = style.find(ns::W, "rPr") {
                parity = parity.xor(Toggles {
                    bold: on_off(rpr, "b") == Some(true),
                    italic: on_off(rpr, "i") == Some(true),
                    strike: on_off(rpr, "strike") == Some(true)
                        || on_off(rpr, "dstrike") == Some(true),
                });
            }
            None
        })?;
        Ok(parity)
    }

    /// Heading level a paragraph style resolves to, from its name
    /// (`heading N`, `Title`) or an `outlineLvl`, inherited through
    /// `basedOn`. Tri-state: `Some(None)` when the nearest specification is
    /// the explicit off value (`outlineLvl` 9), which stops inheritance.
    pub fn heading_level(&self, id: &str) -> Result<Option<Option<u8>>, ConvertError> {
        self.chains.walk(id, |style| {
            let name = style
                .find(ns::W, "name")
                .and_then(|e| e.attr(ns::W, "val"))
                .unwrap_or("")
                .to_ascii_lowercase();
            if let Some(rest) = name.strip_prefix("heading ")
                && let Ok(level) = rest.trim().parse::<u8>()
            {
                return Some(Some(level));
            }
            if name == "title" {
                return Some(Some(1));
            }
            let level = style
                .find(ns::W, "pPr")?
                .find(ns::W, "outlineLvl")?
                .attr(ns::W, "val")?
                .parse::<u8>()
                .ok()?;
            Some(if level < 9 { Some(level + 1) } else { None })
        })
    }

    /// The block container a paragraph style names, inherited through
    /// `basedOn` (Word's `Quote`, Pandoc's `Source Code`, ...).
    pub fn block_style(&self, id: &str) -> Result<Option<BlockStyle>, ConvertError> {
        self.chains.walk(id, |style| {
            let name = style.find(ns::W, "name")?.attr(ns::W, "val")?;
            blockstyle::from_style_name(name)
        })
    }

    /// The `numId` a paragraph style contributes, inherited through
    /// `basedOn`. An `ilvl` inside a style's `numPr` is ignored per ECMA-376
    /// §17.3.1.19 - the effective level comes from the abstract levels'
    /// `w:pStyle` bindings ([`Styles::style_numbering_level`]).
    pub fn style_num_pr(&self, id: &str) -> Result<Option<u64>, ConvertError> {
        self.chains.walk(id, |style| {
            style
                .find(ns::W, "pPr")?
                .find(ns::W, "numPr")?
                .find(ns::W, "numId")
                .and_then(|e| e.attr(ns::W, "val"))?
                .parse()
                .ok()
        })
    }

    /// The numbering level a paragraph style binds to: the first style along
    /// the `basedOn` chain (child first) that one of the instance's abstract
    /// levels references via `w:pStyle`.
    pub fn style_numbering_level(
        &self,
        id: &str,
        instance: &crate::formats::docx::numbering::Instance,
    ) -> Result<Option<usize>, ConvertError> {
        self.chains.walk(id, |style| {
            let style_id = style.attr(ns::W, "styleId")?;
            instance.style_level(style_id)
        })
    }

    /// The `numId` referenced by a numbering style's own `numPr`
    /// (`numStyleLink` contract), without inheritance.
    pub fn direct_num_id(&self, id: &str) -> Option<u64> {
        let style = self.chains.definition(id)?;
        style
            .find(ns::W, "pPr")?
            .find(ns::W, "numPr")?
            .find(ns::W, "numId")?
            .attr(ns::W, "val")?
            .parse()
            .ok()
    }
}

/// A `w:rPr` element as a tri-state delta - used only for *direct* run
/// formatting, where specifications are absolute on/off.
pub fn rpr_delta(rpr: &Element) -> StyleDelta {
    let (s, d) = (on_off(rpr, "strike"), on_off(rpr, "dstrike"));
    StyleDelta {
        bold: on_off(rpr, "b"),
        italic: on_off(rpr, "i"),
        strike: if s.is_some() || d.is_some() {
            Some(s.unwrap_or(false) || d.unwrap_or(false))
        } else {
            None
        },
        code: None,
    }
}

/// ST_OnOff: `1`/`true`/`on` (or no value) are true; `0`/`false`/`off` are
/// false; an absent element is unspecified.
pub fn on_off(parent: &Element, name: &str) -> Option<bool> {
    let elem = parent.find(ns::W, name)?;
    Some(!matches!(elem.attr(ns::W, "val"), Some("0" | "false" | "off" | "none")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::xml::parse_xml;

    // parse_xml returns a synthetic root wrapping the document element.
    fn parse(doc: &str) -> Element {
        parse_xml(doc.as_bytes()).unwrap()
    }

    #[test]
    fn on_off_covers_the_full_value_space() {
        for (xml, expect) in [
            (r#"<w:b/>"#, Some(true)),
            (r#"<w:b w:val="1"/>"#, Some(true)),
            (r#"<w:b w:val="true"/>"#, Some(true)),
            (r#"<w:b w:val="on"/>"#, Some(true)),
            (r#"<w:b w:val="0"/>"#, Some(false)),
            (r#"<w:b w:val="false"/>"#, Some(false)),
            (r#"<w:b w:val="off"/>"#, Some(false)),
            ("", None),
        ] {
            let root = parse(&format!(
                r#"<w:rPr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">{xml}</w:rPr>"#
            ));
            let rpr = root.find(ns::W, "rPr").unwrap();
            assert_eq!(on_off(rpr, "b"), expect, "for {xml:?}");
        }
    }

    #[test]
    fn toggles_flip_the_base_and_double_flips_cancel() {
        let base = Style { bold: true, ..Style::PLAIN };
        let flip = Toggles { bold: true, ..Default::default() };
        assert!(!flip.apply_over(base).bold);
        assert!(flip.xor(flip).apply_over(base).bold);
    }

    #[test]
    fn style_false_contributes_nothing_to_parity() {
        let styles_xml = r#"<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
            <w:style w:type="character" w:styleId="NotBold"><w:rPr><w:b w:val="0"/></w:rPr></w:style>
            <w:style w:type="character" w:styleId="Flip"><w:basedOn w:val="NotBold"/><w:rPr><w:b/></w:rPr></w:style>
        </w:styles>"#;
        let root = parse(styles_xml);
        let styles = Styles::parse(root.find(ns::W, "styles").unwrap());
        assert_eq!(styles.run_toggles("NotBold").unwrap(), Toggles::default());
        assert!(styles.run_toggles("Flip").unwrap().bold);
    }
}
