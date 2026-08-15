//! OOXML markup-compatibility (`mc:AlternateContent`) branch selection.

use crate::package::xml::{Element, ns};

/// Pick the branch of an `mc:AlternateContent` to process: the first
/// `mc:Choice` whose `Requires` namespaces are all supported, else the
/// `mc:Fallback`.
pub fn alternate_branch<'e>(alt: &'e Element, supported: &[&str]) -> Option<&'e Element> {
    alt.find_all(ns::MC, "Choice")
        .find(|c| choice_supported(c, supported))
        .or_else(|| alt.find(ns::MC, "Fallback"))
}

fn choice_supported(choice: &Element, supported: &[&str]) -> bool {
    let Some(requires) = choice.attr(ns::MC, "Requires") else {
        return true;
    };
    // Requires prefixes were resolved to namespace URIs at parse time, in
    // the element's own lexical scope; an unresolved prefix stays literal
    // and matches nothing.
    requires.split_whitespace().all(|uri| supported.contains(&uri))
}
