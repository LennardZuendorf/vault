use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ItemDetails {
    pub display_name: Option<String>,
    pub tags: Vec<String>,
    pub color: Option<ItemColor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ItemColor {
    #[serde(rename = "data-1")]
    Data1,
    #[serde(rename = "data-2")]
    Data2,
    #[serde(rename = "data-3")]
    Data3,
    #[serde(rename = "data-4")]
    Data4,
    #[serde(rename = "data-5")]
    Data5,
    #[serde(rename = "data-6")]
    Data6,
}

impl ItemDetails {
    /// Validate raw text before persisting or publishing details.
    pub fn normalize(self) -> Result<Self> {
        Ok(Self {
            display_name: normalize_display_name(self.display_name.as_deref())?,
            tags: normalize_tags(&self.tags)?,
            color: self.color,
        })
    }
}

/// A blank name clears the override.
pub fn normalize_display_name(raw: Option<&str>) -> Result<Option<String>> {
    match raw {
        Some(raw) => normalize_text(raw, 120, "Display name"),
        None => Ok(None),
    }
}

/// Assignment lists ignore blanks; single-tag callers must reject `None`.
pub fn normalize_tag(raw: &str) -> Result<Option<String>> {
    normalize_text(raw, 40, "Tag")
}

/// Keep first-assigned spelling here; catalog spelling is applied by the store.
pub fn normalize_tags(raw: &[String]) -> Result<Vec<String>> {
    let mut seen = HashSet::new();
    let mut tags = Vec::new();
    for raw_tag in raw {
        if let Some(tag) = normalize_tag(raw_tag)? {
            if seen.insert(tag_identity(&tag)) {
                tags.push(tag);
                if tags.len() > 12 {
                    return Err(Error::Other("Too many tags. Assign at most 12 distinct tags.".into()));
                }
            }
        }
    }
    Ok(tags)
}

/// Compare already-normalized tags without depending on the system locale.
pub fn tag_identity(tag: &str) -> String {
    tag.to_lowercase()
}

fn normalize_text(raw: &str, max_chars: usize, field: &str) -> Result<Option<String>> {
    // Trimming first would silently accept forbidden controls at the edges.
    if raw.chars().any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}')) {
        return Err(Error::Other(format!(
            "{field} contains a control character or line separator. Use single-line text without control characters."
        )));
    }
    let text = raw.trim();
    if text.chars().count() > max_chars {
        return Err(Error::Other(format!("{field} is too long. Use at most {max_chars} Unicode characters.")));
    }
    Ok((!text.is_empty()).then(|| text.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn missing_fields_load_without_details() {
        let details: ItemDetails = serde_json::from_value(json!({})).unwrap();
        assert_eq!(details.display_name, None);
        assert!(details.tags.is_empty());
        assert_eq!(details.color, None);
        assert_eq!(serde_json::to_value(details).unwrap(), json!({"display_name": null, "tags": [], "color": null}));
    }

    #[test]
    fn names_trim_unicode_whitespace_and_blank_names_clear() {
        assert_eq!(normalize_display_name(Some(" \u{a0}Focus\u{2003} ")).unwrap(), Some("Focus".into()));
        assert_eq!(normalize_display_name(Some(" \u{2003} ")).unwrap(), None);
        assert_eq!(normalize_display_name(Some("")).unwrap(), None);
        assert_eq!(normalize_display_name(None).unwrap(), None);
    }

    #[test]
    fn tags_trim_unicode_whitespace_and_ignore_blanks() {
        assert_eq!(normalize_tag(" \u{a0}Review\u{2003} ").unwrap(), Some("Review".into()));
        assert_eq!(normalize_tag(" \u{2003} ").unwrap(), None);
        assert_eq!(normalize_tag("").unwrap(), None);
    }

    #[test]
    fn duplicate_tags_keep_first_spelling_and_assignment_order() {
        let tags = [" Beta ", "", "ALPHA", "beta", "alpha", " Gamma ", " "].map(String::from);
        assert_eq!(normalize_tags(&tags).unwrap(), ["Beta", "ALPHA", "Gamma"]);
    }

    #[test]
    fn unicode_tag_identity_is_locale_independent_lowercase() {
        assert_eq!(tag_identity("ÄBC"), "äbc");
        assert_eq!(tag_identity("İ"), "i\u{307}");
        assert_eq!(normalize_tags(&["ÄBC".into(), "äbc".into()]).unwrap(), ["ÄBC"]);
    }

    #[test]
    fn name_limit_counts_unicode_scalars_after_trimming() {
        let limit = "🦀".repeat(120);
        assert_eq!(normalize_display_name(Some(&format!(" {limit} "))).unwrap(), Some(limit));
        assert!(normalize_display_name(Some(&"🦀".repeat(121))).is_err());
        assert!(normalize_display_name(Some(&"e\u{301}".repeat(61))).is_err());
    }

    #[test]
    fn tag_limit_counts_unicode_scalars_after_trimming() {
        let limit = "🦀".repeat(40);
        assert_eq!(normalize_tag(&format!(" {limit} ")).unwrap(), Some(limit));
        assert!(normalize_tag(&"🦀".repeat(41)).is_err());
        assert!(normalize_tag(&"e\u{301}".repeat(21)).is_err());
    }

    #[test]
    fn tag_count_limit_applies_after_ignoring_blanks_and_duplicates() {
        let mut tags: Vec<String> = (1..=12).map(|i| format!("tag {i}")).collect();
        assert_eq!(normalize_tags(&tags).unwrap().len(), 12);
        tags.extend([" TAG 1 ".into(), "".into(), " ".into()]);
        assert_eq!(normalize_tags(&tags).unwrap().len(), 12);
        tags.push("tag 13".into());
        assert!(normalize_tags(&tags).is_err());
    }

    #[test]
    fn raw_controls_and_line_separators_are_rejected_before_trimming() {
        for control in ['\n', '\r', '\t', '\0', '\u{7f}', '\u{85}', '\u{2028}', '\u{2029}'] {
            for raw in
                [format!("{control}Valid"), format!("Valid{control}"), format!("Va{control}lid"), control.to_string()]
            {
                assert!(normalize_display_name(Some(&raw)).is_err(), "name accepted {raw:?}");
                assert!(normalize_tag(&raw).is_err(), "tag accepted {raw:?}");
                assert!(normalize_tags(std::slice::from_ref(&raw)).is_err(), "tags accepted {raw:?}");
            }
        }
    }

    #[test]
    fn invalid_tags_are_rejected_even_if_they_exceed_the_distinct_tag_limit() {
        let mut tags: Vec<String> = (1..=12).map(|i| format!("tag {i}")).collect();
        tags.push("\n".into());
        assert!(normalize_tags(&tags).is_err());
    }

    #[test]
    fn details_normalization_applies_to_each_text_field_and_preserves_color() {
        let details = ItemDetails {
            display_name: Some(" Focus ".into()),
            tags: vec![" Review ".into(), "review".into(), "".into()],
            color: Some(ItemColor::Data3),
        };
        assert_eq!(
            details.normalize().unwrap(),
            ItemDetails {
                display_name: Some("Focus".into()),
                tags: vec!["Review".into()],
                color: Some(ItemColor::Data3),
            }
        );
        assert_eq!(
            ItemDetails { display_name: Some(" ".into()), ..ItemDetails::default() }.normalize().unwrap(),
            ItemDetails::default()
        );
    }

    #[test]
    fn details_normalization_rejects_invalid_fields() {
        let invalid_name = ItemDetails { display_name: Some("bad\nname".into()), ..ItemDetails::default() };
        assert!(invalid_name.normalize().is_err());
        let invalid_tag = ItemDetails { tags: vec!["bad\ttag".into()], ..ItemDetails::default() };
        assert!(invalid_tag.normalize().is_err());
    }

    #[test]
    fn palette_values_round_trip_as_exact_kebab_case() {
        for (color, wire) in [
            (ItemColor::Data1, "data-1"),
            (ItemColor::Data2, "data-2"),
            (ItemColor::Data3, "data-3"),
            (ItemColor::Data4, "data-4"),
            (ItemColor::Data5, "data-5"),
            (ItemColor::Data6, "data-6"),
        ] {
            assert_eq!(serde_json::to_value(color).unwrap(), json!(wire));
            assert_eq!(serde_json::from_value::<ItemColor>(json!(wire)).unwrap(), color);
        }
    }

    #[test]
    fn colors_outside_the_palette_are_rejected_at_the_input_boundary() {
        for color in ["data-0", "data-7", "Data1", "data1", "DATA-1", " data-1 ", "#abcdef", "lavender", ""] {
            assert!(serde_json::from_value::<ItemDetails>(json!({"color": color})).is_err(), "accepted {color:?}");
        }
    }
}
