//! Style-chain traversal: walk a parent (`basedOn`) chain child-to-root.
//! A cycle is a resource-safety bound and hard-fails - recovery never
//! weakens resource safety.

use crate::error::ConvertError;
use std::collections::{HashMap, HashSet};

/// Style id -> (definition, parent style id).
pub struct StyleChains<'a, D> {
    raw: HashMap<&'a str, (&'a D, Option<&'a str>)>,
}

impl<'a, D> Default for StyleChains<'a, D> {
    fn default() -> Self {
        StyleChains { raw: HashMap::new() }
    }
}

impl<'a, D> StyleChains<'a, D> {
    pub fn insert(&mut self, id: &'a str, def: &'a D, parent: Option<&'a str>) {
        self.raw.insert(id, (def, parent));
    }

    pub fn definition(&self, id: &str) -> Option<&'a D> {
        self.raw.get(id).map(|(d, _)| *d)
    }

    /// Walk a chain child-to-root, visiting each definition until `visit`
    /// returns `Some`. Unknown ids end the walk (a dangling reference is
    /// producer sloppiness, not fatal); a cycle hard-fails.
    pub fn walk<T>(
        &self,
        id: &str,
        mut visit: impl FnMut(&'a D) -> Option<T>,
    ) -> Result<Option<T>, ConvertError> {
        let mut visited: HashSet<&str> = HashSet::new();
        let mut cursor = self.raw.get_key_value(id).map(|(k, _)| *k);
        while let Some(current) = cursor {
            if !visited.insert(current) {
                return Err(ConvertError::malformed(format!(
                    "style inheritance cycle at {current:?}"
                )));
            }
            let (def, parent) = self.raw[current];
            if let Some(hit) = visit(def) {
                return Ok(Some(hit));
            }
            cursor = parent.and_then(|p| self.raw.get_key_value(p).map(|(k, _)| *k));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_chain_walks_to_root() {
        let defs: Vec<(String, Option<bool>)> =
            (0..30).map(|i| (format!("s{i}"), if i == 0 { Some(true) } else { None })).collect();
        let mut chains: StyleChains<Option<bool>> = StyleChains::default();
        for (i, (id, d)) in defs.iter().enumerate() {
            let parent = if i == 0 { None } else { Some(defs[i - 1].0.as_str()) };
            chains.insert(id, d, parent);
        }
        let hit = chains.walk("s29", |d| *d).unwrap();
        assert_eq!(hit, Some(true));
    }

    #[test]
    fn child_setting_wins_over_parent() {
        let on = Some(true);
        let off = Some(false);
        let mut chains: StyleChains<Option<bool>> = StyleChains::default();
        chains.insert("base", &on, None);
        chains.insert("kid", &off, Some("base"));
        assert_eq!(chains.walk("kid", |d| *d).unwrap(), Some(false));
    }

    #[test]
    fn cycle_is_hard_error() {
        let none: Option<bool> = None;
        let mut chains: StyleChains<Option<bool>> = StyleChains::default();
        chains.insert("a", &none, Some("b"));
        chains.insert("b", &none, Some("a"));
        assert!(chains.walk("a", |d| *d).is_err());
    }

    #[test]
    fn dangling_parent_ends_the_walk() {
        let on = Some(true);
        let mut chains: StyleChains<Option<bool>> = StyleChains::default();
        chains.insert("kid", &on, Some("ghost"));
        assert_eq!(chains.walk("kid", |d| *d).unwrap(), Some(true));
    }

    #[test]
    fn visits_every_definition_when_probe_never_hits() {
        let a = Some(true);
        let b = Some(false);
        let mut chains: StyleChains<Option<bool>> = StyleChains::default();
        chains.insert("base", &b, None);
        chains.insert("kid", &a, Some("base"));
        let mut seen = 0;
        chains
            .walk::<()>("kid", |_| {
                seen += 1;
                None
            })
            .unwrap();
        assert_eq!(seen, 2);
    }
}
