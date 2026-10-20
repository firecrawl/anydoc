//! WordprocessingML numbering: `numId -> w:num` (with level overrides) `->
//! abstractNum` (with `numStyleLink` indirection) `-> level`, plus the
//! document-order counters that produce each paragraph's effective number.

use crate::error::ConvertError;
use crate::package::xml::{Element, ns};
use crate::shared::list::MarkerKind;
use crate::shared::numbering::{NumberPattern, composite_label, parse_percent_pattern};
use std::collections::HashMap;

pub const LEVELS: usize = 9;

#[derive(Debug, Clone)]
pub struct LevelDef {
    /// `None` = suppressed numbering (`numFmt` of `none`).
    pub marker: Option<MarkerKind>,
    pub start: u64,
    /// `w:lvlRestart`: `None` = restart when any shallower level appears,
    /// `Some(0)` = never restart, `Some(n)` = restart when a level with
    /// `ilvl < n` appears.
    pub restart: Option<u32>,
    /// `w:lvlText` (with `w:isLgl`) as the shared pattern IR.
    pub pattern: NumberPattern,
}

impl Default for LevelDef {
    fn default() -> Self {
        LevelDef {
            marker: Some(MarkerKind::Bullet),
            start: 1,
            restart: None,
            pattern: NumberPattern::default(),
        }
    }
}

#[derive(Debug, Clone)]
struct AbstractNum {
    levels: [LevelDef; LEVELS],
    /// Paragraph style each level binds to via `w:lvl > w:pStyle`.
    pstyles: [Option<String>; LEVELS],
    num_style_link: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Instance {
    pub levels: [LevelDef; LEVELS],
    pstyles: [Option<String>; LEVELS],
    /// Identity of the abstract definition this instance resolves to (after
    /// any numStyleLink indirection). Instances sharing an abstract are one
    /// logical list in Word, so their counters continue across instance
    /// switches (#96).
    pub(crate) abstract_key: u64,
    /// Levels this instance overrides (`lvlOverride` with a nested `w:lvl` or
    /// a `startOverride`). Entering the list through such an instance restarts
    /// exactly those levels; every other level keeps counting (#96).
    pub(crate) overrides: [bool; LEVELS],
}

impl Instance {
    /// The level bound to a paragraph style through the abstract levels'
    /// `w:pStyle` references.
    pub fn style_level(&self, style_id: &str) -> Option<usize> {
        self.pstyles.iter().position(|p| p.as_deref() == Some(style_id))
    }
}

#[derive(Debug, Default)]
pub struct Numbering {
    instances: HashMap<u64, Instance>,
}

impl Numbering {
    pub fn instance(&self, num_id: u64) -> Option<&Instance> {
        self.instances.get(&num_id)
    }
}

/// `style_num_id` maps a numbering style id to the `numId` its own `numPr`
/// references (the `numStyleLink`/`styleLink` indirection contract).
pub fn parse(
    root: &Element,
    style_num_id: &impl Fn(&str) -> Option<u64>,
) -> Result<Numbering, ConvertError> {
    let mut abstracts: HashMap<&str, AbstractNum> = HashMap::new();
    for abs in root.find_all(ns::W, "abstractNum") {
        let Some(id) = abs.attr(ns::W, "abstractNumId") else {
            continue;
        };
        let mut levels: [LevelDef; LEVELS] = std::array::from_fn(|_| LevelDef::default());
        let mut pstyles: [Option<String>; LEVELS] = Default::default();
        for lvl in abs.find_all(ns::W, "lvl") {
            let ilvl: usize = lvl.attr(ns::W, "ilvl").and_then(|v| v.parse().ok()).unwrap_or(0);
            if ilvl < LEVELS {
                levels[ilvl] = parse_level(lvl);
                pstyles[ilvl] = level_pstyle(lvl);
            }
        }
        let num_style_link =
            abs.find(ns::W, "numStyleLink").and_then(|e| e.attr(ns::W, "val")).map(str::to_string);
        abstracts.insert(id, AbstractNum { levels, pstyles, num_style_link });
    }

    // First pass: direct abstract references per numId.
    let mut direct: HashMap<u64, (&str, &Element)> = HashMap::new();
    for num in root.find_all(ns::W, "num") {
        let (Some(num_id), Some(abs_id)) = (
            num.attr(ns::W, "numId").and_then(|v| v.parse::<u64>().ok()),
            num.find(ns::W, "abstractNumId").and_then(|e| e.attr(ns::W, "val")),
        ) else {
            continue;
        };
        direct.insert(num_id, (abs_id, num));
    }

    // Stable identity per abstract definition (document order). Counters key
    // on this so instances that share an abstract continue one logical list.
    let mut abstract_keys: HashMap<&str, u64> = HashMap::new();
    for abs in root.find_all(ns::W, "abstractNum") {
        let Some(id) = abs.attr(ns::W, "abstractNumId") else { continue };
        let key = abstract_keys.len() as u64;
        abstract_keys.entry(id).or_insert(key);
    }

    let mut numbering = Numbering::default();
    for (&num_id, &(abs_id, num_elem)) in &direct {
        let Some((resolved_id, abs)) = resolve_abstract(abs_id, &abstracts, &direct, style_num_id)?
        else {
            log::warn!("numbering instance {num_id} references unknown abstract {abs_id:?}");
            continue;
        };
        let mut levels = abs.levels.clone();
        let mut pstyles = abs.pstyles.clone();
        let mut overrides = [false; LEVELS];
        for over in num_elem.find_all(ns::W, "lvlOverride") {
            let ilvl: usize = over.attr(ns::W, "ilvl").and_then(|v| v.parse().ok()).unwrap_or(0);
            if ilvl >= LEVELS {
                continue;
            }
            // A nested w:lvl replaces the level wholesale; startOverride is
            // applied last so it survives the replacement. Both count as an
            // override: entering the list through this instance restarts the
            // level instead of continuing the shared sequence (#96).
            let mut overridden = false;
            if let Some(lvl) = over.find(ns::W, "lvl") {
                levels[ilvl] = parse_level(lvl);
                pstyles[ilvl] = level_pstyle(lvl);
                overridden = true;
            }
            if let Some(start) = over
                .find(ns::W, "startOverride")
                .and_then(|e| e.attr(ns::W, "val"))
                .and_then(parse_start)
            {
                levels[ilvl].start = start;
                overridden = true;
            }
            overrides[ilvl] = overridden;
        }
        numbering.instances.insert(
            num_id,
            Instance {
                levels,
                pstyles,
                abstract_key: abstract_keys.get(resolved_id.as_str()).copied().unwrap_or(u64::MAX),
                overrides,
            },
        );
    }
    Ok(numbering)
}

/// Resolve an abstract definition, following `numStyleLink` indirection:
/// abstract -> numbering style -> that style's `numId` -> its abstract.
fn resolve_abstract<'n>(
    abs_id: &str,
    abstracts: &'n HashMap<&str, AbstractNum>,
    direct: &HashMap<u64, (&str, &Element)>,
    style_num_id: &impl Fn(&str) -> Option<u64>,
) -> Result<Option<(String, &'n AbstractNum)>, ConvertError> {
    let mut seen: Vec<String> = Vec::new();
    let mut current = abs_id.to_string();
    loop {
        if seen.contains(&current) {
            return Err(ConvertError::malformed(format!(
                "numbering indirection cycle at abstract {current:?}"
            )));
        }
        seen.push(current.clone());
        let Some(abs) = abstracts.get(current.as_str()) else {
            return Ok(None);
        };
        let Some(style_id) = &abs.num_style_link else {
            return Ok(Some((current.clone(), abs)));
        };
        let linked = style_num_id(style_id)
            .and_then(|num_id| direct.get(&num_id))
            .map(|(abs_id, _)| abs_id.to_string());
        match linked {
            Some(next) => current = next,
            None => return Ok(Some((current.clone(), abs))),
        }
    }
}

/// The paragraph style a `w:lvl` binds to (`w:pStyle`).
fn level_pstyle(lvl: &Element) -> Option<String> {
    lvl.find(ns::W, "pStyle").and_then(|e| e.attr(ns::W, "val")).map(str::to_string)
}

/// `ST_DecimalNumber` is `xsd:int`; values are clamped to the non-negative
/// range so untrusted starting values can never overflow counters.
fn parse_start(v: &str) -> Option<u64> {
    v.parse::<i64>().ok().map(|n| n.clamp(0, i32::MAX as i64) as u64)
}

fn parse_level(lvl: &Element) -> LevelDef {
    let fmt = lvl.find(ns::W, "numFmt").and_then(|e| e.attr(ns::W, "val")).unwrap_or("bullet");
    let marker = match fmt {
        "none" => None,
        "bullet" => Some(MarkerKind::Bullet),
        "lowerLetter" => Some(MarkerKind::LowerAlpha),
        "upperLetter" => Some(MarkerKind::UpperAlpha),
        "lowerRoman" => Some(MarkerKind::LowerRoman),
        "upperRoman" => Some(MarkerKind::UpperRoman),
        _ => Some(MarkerKind::Decimal),
    };
    let start = lvl
        .find(ns::W, "start")
        .and_then(|e| e.attr(ns::W, "val"))
        .and_then(parse_start)
        .unwrap_or(1);
    let restart = lvl
        .find(ns::W, "lvlRestart")
        .and_then(|e| e.attr(ns::W, "val"))
        .and_then(|v| v.parse().ok());
    // The number text pattern applies to ordered levels; bullet lvlText is
    // the glyph itself, not a pattern.
    let text = match marker {
        Some(m) if m.ordered() => lvl
            .find(ns::W, "lvlText")
            .and_then(|e| e.attr(ns::W, "val"))
            .map(parse_percent_pattern)
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let legal = crate::formats::docx::styles::on_off(lvl, "isLgl") == Some(true);
    LevelDef { marker, start, restart, pattern: NumberPattern { text, legal } }
}

/// Document-order numbering state: one counter array per instance, shared
/// across interruptions so continuation works, with `lvlRestart` semantics.
#[derive(Default)]
pub struct Counters {
    state: HashMap<u64, InstanceState>,
}

#[derive(Default)]
struct InstanceState {
    value: [u64; LEVELS],
    initialized: [bool; LEVELS],
    restart_pending: [bool; LEVELS],
    /// The instance currently driving this abstract (`u64::MAX` = none yet;
    /// real `numId`s never reach the counters as `u64::MAX`). Switching
    /// instances inside one logical list restarts only the levels the
    /// entering instance overrides (#96). `None` until the list's first
    /// paragraph; `Some` never collides with a real id because it is
    /// compared as an `Option`, not against a sentinel value.
    last_num: Option<u64>,
    /// The overridden levels of the currently active instance. Each restarts
    /// on its first USE under this instance — not eagerly at the switch, so a
    /// following plain instance cannot consume a restart that was never
    /// earned (cubic review of #129).
    fresh_overrides: [bool; LEVELS],
}

impl Counters {
    /// Advance the counter for a paragraph at (`num_id`, `ilvl`) and return
    /// its effective number plus, when the level's number text is not
    /// reproducible from the marker kind alone, the composite label.
    pub fn next(&mut self, num_id: u64, ilvl: usize, instance: &Instance) -> (u64, Option<String>) {
        let ilvl = ilvl.min(LEVELS - 1);
        let state = self.state.entry(instance.abstract_key).or_default();
        if state.last_num != Some(num_id) {
            // First paragraph of this list, or an instance switch within it.
            state.fresh_overrides = instance.overrides;
            state.last_num = Some(num_id);
        }
        let def = &instance.levels[ilvl];
        let override_restart = std::mem::take(&mut state.fresh_overrides[ilvl]);
        if !state.initialized[ilvl] || state.restart_pending[ilvl] || override_restart {
            state.value[ilvl] = def.start;
            state.initialized[ilvl] = true;
            state.restart_pending[ilvl] = false;
        } else {
            state.value[ilvl] = state.value[ilvl].saturating_add(1);
        }
        // Using level `ilvl` schedules restarts for deeper levels according
        // to their lvlRestart settings.
        for (deeper, level_def) in instance.levels.iter().enumerate().skip(ilvl + 1) {
            let triggered = match level_def.restart {
                None => true,
                Some(0) => false,
                Some(n) => (ilvl as u32) < n,
            };
            if triggered {
                state.restart_pending[deeper] = true;
            }
        }
        let value = state.value[ilvl];
        let label = def.marker.and_then(|marker| {
            composite_label(
                &def.pattern,
                marker,
                value,
                |l| instance.levels[l.min(LEVELS - 1)].marker.unwrap_or(MarkerKind::Decimal),
                |l| {
                    let l = l.min(LEVELS - 1);
                    if state.initialized[l] && !state.restart_pending[l] {
                        state.value[l]
                    } else {
                        instance.levels[l].start
                    }
                },
            )
        });
        (value, label)
    }
}
