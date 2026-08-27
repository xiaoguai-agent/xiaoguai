//! Shared quick-xml helpers for the Office Open XML loaders (`docx`, `pptx`).

/// Resolve a [`quick_xml::events::Event::GeneralRef`] entity name into its text.
///
/// quick-xml 0.42 stopped delivering entity references inside `Event::Text` and
/// began emitting them as their own `GeneralRef` event carrying the bare name
/// (`amp`, `#38`, `#x26` — no `&` or `;`). A loader that only handles
/// `Event::Text` therefore drops every entity silently, which is exactly how
/// the 0.41 → 0.42 bump would have shipped: the fixtures contain no entities,
/// so every existing test still passed.
///
/// Reconstructing `&name;` and running it back through `escape::unescape` reuses
/// the library's own resolution, covering predefined entities and numeric
/// character references alike. An unresolvable name is emitted verbatim rather
/// than dropped — losing the text silently is the failure mode this exists to
/// prevent.
#[must_use]
pub(super) fn resolve_entity(name: &str) -> String {
    let raw = format!("&{name};");
    match quick_xml::escape::unescape(&raw) {
        Ok(text) => text.into_owned(),
        Err(_) => raw,
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_entity;

    #[test]
    fn resolves_the_five_predefined_entities() {
        assert_eq!(resolve_entity("amp"), "&");
        assert_eq!(resolve_entity("lt"), "<");
        assert_eq!(resolve_entity("gt"), ">");
        assert_eq!(resolve_entity("quot"), "\"");
        assert_eq!(resolve_entity("apos"), "'");
    }

    #[test]
    fn resolves_decimal_and_hex_character_references() {
        assert_eq!(resolve_entity("#38"), "&");
        assert_eq!(resolve_entity("#x26"), "&");
        assert_eq!(resolve_entity("#x4E2D"), "中");
    }

    #[test]
    fn unknown_entity_is_kept_verbatim_not_dropped() {
        assert_eq!(resolve_entity("nbsp"), "&nbsp;");
    }
}
