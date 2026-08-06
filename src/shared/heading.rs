//! Shared heading identity rules used by paragraph-style frontends.

/// Resolve a built-in paragraph style name to an outline level.
pub fn level_from_name(name: &str) -> Option<u8> {
    let name = name.trim().to_ascii_lowercase();
    if let Some(rest) = name.strip_prefix("heading ")
        && let Ok(level) = rest.trim().parse::<u8>()
    {
        return Some(level);
    }
    match name.as_str() {
        "title" => Some(1),
        "subtitle" => Some(2),
        _ => None,
    }
}

/// Resolve a normalized style id, keeping the match anchored so names such
/// as `Heading_Box` and `Heading10` cannot masquerade as built-in headings.
pub fn level_from_style_id(style_id: &str) -> Option<u8> {
    let normalized = style_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    if let Some(rest) = normalized.strip_prefix("heading")
        && rest.len() == 1
        && let Some(level) = rest.chars().next().and_then(|c| c.to_digit(10))
    {
        return Some(level as u8);
    }
    level_from_name(&normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_anchored_and_built_ins_are_supported() {
        assert_eq!(level_from_name(" Heading 2 "), Some(2));
        assert_eq!(level_from_name("Title"), Some(1));
        assert_eq!(level_from_name("Subtitle"), Some(2));
        assert_eq!(level_from_name("Heading Box"), None);
    }

    #[test]
    fn style_ids_are_normalized_but_anchored() {
        assert_eq!(level_from_style_id("Heading_3"), Some(3));
        assert_eq!(level_from_style_id("Heading10"), None);
        assert_eq!(level_from_style_id("Heading_Box"), None);
    }
}
