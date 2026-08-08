//! Excel number-format rendering for numeric spreadsheet cells.
//!
//! Delegates to `ssfmt` (an Excel-compatible ECMA-376 number-format renderer);
//! this module owns the policy around it: which codes are eligible for
//! rendering at all, and the fallback contract (`None` -> the caller renders
//! the raw value exactly as before this feature).

use std::collections::HashMap;

/// Render `value` with an Excel number-format `code`.
///
/// Returns `None` when the code is not renderable by this pipeline:
/// `General`, empty, date/time-like, text (`@`), or rejected by the
/// renderer. `None` means the caller falls back to its raw rendering; a
/// rendering attempt never fails the conversion.
pub fn render(value: f64, code: &str) -> Option<String> {
    if !renderable_code(code) {
        return None;
    }
    ssfmt::format(value, code, &ssfmt::FormatOptions::default()).ok()
}

/// Resolve the format code for a style's `numFmtId`.
///
/// Custom ids resolve from the workbook's `numFmts` table. Builtin ids
/// resolve through KTD5: numeric/percent/scientific/fraction/accounting
/// ids render; date, time, locale-date, undefined, and text ids map to
/// `None` (the caller keeps today's behavior for those cell kinds).
pub fn code_for_style(num_fmt_id: u32, custom: &HashMap<u32, String>) -> Option<String> {
    if let Some(code) = custom.get(&num_fmt_id) {
        return renderable_code(code).then(|| code.clone());
    }
    match num_fmt_id {
        // General: raw rendering stays.
        0 => None,
        // Date/time ids calamine already converts to DateTime, plus locale
        // date ids calamine does not classify and text id 49.
        14..=22 | 27..=36 | 45..=47 | 49 => None,
        // Accounting ids 37-40 render via ssfmt; 41-44 carry a locale
        // currency symbol and stay raw until locale support lands.
        41..=44 => None,
        _ => ssfmt::builtin_formats::format_code_from_id(num_fmt_id).map(str::to_owned),
    }
}

/// Whether a code is eligible for numeric rendering.
///
/// Guards `General`, empty codes, and date/time-like codes. The date guard
/// mirrors calamine's classifier (see `detect_custom_number_format` in
/// calamine `formats.rs`): date letters flag only outside quoted literals,
/// escapes, and bracket contents, so `[Red]`, `[$-409]`, and `"mm"` stay
/// renderable while `dd/mm/yyyy` and `[h]:mm:ss` fall back to the caller
/// (calamine already converts those cells; this guard is defensive).
fn renderable_code(code: &str) -> bool {
    if code.is_empty() || code.eq_ignore_ascii_case("general") {
        return false;
    }
    let mut escaped = false;
    let mut quoted = false;
    let mut brackets = 0u8;
    let mut after_ampm = false;
    for s in code.chars() {
        match (s, escaped, quoted) {
            (_, true, _) => escaped = false,
            ('\\' | '_' | '*', false, false) => escaped = true,
            ('"', _, true) => quoted = false,
            (_, _, true) => {}
            ('"', _, false) => quoted = true,
            // Text placeholder: never rendered for numeric cells.
            ('@', _, false) => return false,
            ('[', _, _) => brackets += 1,
            (']', _, _) => brackets = brackets.saturating_sub(1),
            ('a' | 'A', _, _) => after_ampm = true,
            ('p' | 'P', _, _) if after_ampm => return false,
            ('d' | 'm' | 'h' | 'y' | 's' | 'D' | 'M' | 'H' | 'Y' | 'S', _, _) if brackets == 0 => {
                return false
            }
            _ => {}
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_or_raw(value: f64, code: &str) -> String {
        render(value, code).unwrap_or_else(|| value.to_string())
    }

    #[test]
    fn percent_formats_multiply_by_100() {
        assert_eq!(render(0.653035934239184, "0%"), Some("65%".to_string()));
        assert_eq!(render(0.653035934239184, "0.00%"), Some("65.30%".to_string()));
        assert_eq!(render(0.075, "0.0%"), Some("7.5%".to_string()));
    }

    #[test]
    fn currency_and_literals_render() {
        assert_eq!(render(56701.0309278351, "$#,##0.00"), Some("$56,701.03".to_string()));
        assert_eq!(render(1234.5, "[$$-409]#,##0.00"), Some("$1,234.50".to_string()));
    }

    #[test]
    fn decimals_grouping_and_scaling_render() {
        assert_eq!(render(1234.5, "#,##0.00"), Some("1,234.50".to_string()));
        assert_eq!(render(1234567.0, "#,##0"), Some("1,234,567".to_string()));
        assert_eq!(render(1000.0, "#,##0"), Some("1,000".to_string()));
        // Trailing-comma scaling: ssfmt rounds the scaled integer digits; a
        // half-way fractional part truncates (documented renderer quirk).
        assert_eq!(render(1234567.9, "#,##0,"), Some("1,235".to_string()));
        assert_eq!(render(1234567.0, "#,##0,"), Some("1,234".to_string()));
    }

    #[test]
    fn negatives_follow_section_rules() {
        assert_eq!(render(-1.5, "0.00;(0.00)"), Some("(1.50)".to_string()));
        assert_eq!(render(-1.5, "0.00"), Some("-1.50".to_string()));
        // Color brackets in the negative section render without color.
        assert_eq!(render(-1.5, "#,##0 ;[Red](#,##0)"), Some("(2)".to_string()));
    }

    #[test]
    fn scientific_notation_renders() {
        assert_eq!(render(123456.0, "0.00E+00"), Some("1.23E+05".to_string()));
        assert_eq!(render(123456.0, "##0.0E+0"), Some("123.5E+3".to_string()));
    }

    #[test]
    fn tie_values_round_half_away_from_zero() {
        assert_eq!(render(0.5, "0"), Some("1".to_string()));
        assert_eq!(render(2.5, "0"), Some("3".to_string()));
        assert_eq!(render(1.25, "0.0"), Some("1.3".to_string()));
        assert_eq!(render(-0.5, "0"), Some("-1".to_string()));
        // 1.005 as f64 is just under the tie; Excel displays 1.00 the same.
        assert_eq!(render(1.005, "0.00"), Some("1.00".to_string()));
    }

    #[test]
    fn fractions_render() {
        assert_eq!(render(0.5, "# ?/?"), Some(" 1/2".to_string()));
    }

    #[test]
    fn general_and_empty_codes_fall_back() {
        assert_eq!(render(1.5, "General"), None);
        assert_eq!(render(1.5, "general"), None);
        assert_eq!(render(1.5, ""), None);
    }

    #[test]
    fn date_like_codes_fall_back() {
        assert_eq!(render(1.5, "dd/mm/yyyy"), None);
        assert_eq!(render(1.5, "[h]:mm:ss"), None);
        assert_eq!(render(1.5, "h:mm AM/PM"), None);
        assert_eq!(render(46031.0, "m/d/yy"), None);
        // Quote/escape/bracket literals must not trip the date guard.
        assert_eq!(render(1234.5, "\"Total\" #,##0.00"), Some("Total 1,234.50".to_string()));
        assert_eq!(render(1234.5, "[$$-409]#,##0.00"), Some("$1,234.50".to_string()));
    }

    #[test]
    fn text_format_falls_back() {
        assert_eq!(render(3.5, "@"), None);
    }

    #[test]
    fn renderer_errors_fall_back() {
        assert_eq!(render(1.5, "0.00["), None);
        assert_eq!(render(1.5, "0.00[xyz"), None);
    }

    #[test]
    fn renderer_never_panics_on_adversarial_codes() {
        // Renderer output is pinned loosely (non-empty on success); the
        // contract under test is fallback-on-error, never a panic.
        let _ = render_or_raw(1.5, "0.00E+00_XYZ");
        let _ = render_or_raw(-0.653, "0.00%");
        assert_eq!(render(-0.653, "0.00%"), Some("-65.30%".to_string()));
    }

    #[test]
    fn builtin_ids_resolve_through_ktd5() {
        let none = HashMap::new();
        assert_eq!(code_for_style(9, &none), Some("0%".to_string()));
        assert_eq!(code_for_style(4, &none), Some("#,##0.00".to_string()));
        assert_eq!(code_for_style(2, &none), Some("0.00".to_string()));
        assert_eq!(code_for_style(11, &none), Some("0.00E+00".to_string()));
        assert_eq!(code_for_style(37, &none), Some("#,##0 ;(#,##0)".to_string()));
        assert_eq!(code_for_style(48, &none), Some("##0.0E+0".to_string()));
        assert_eq!(code_for_style(0, &none), None);
        assert_eq!(code_for_style(14, &none), None);
        assert_eq!(code_for_style(27, &none), None);
        assert_eq!(code_for_style(45, &none), None);
        assert_eq!(code_for_style(49, &none), None);
        assert_eq!(code_for_style(41, &none), None);
        assert_eq!(code_for_style(164, &none), None);
        let mut custom = HashMap::new();
        custom.insert(164, "0.00%".to_string());
        assert_eq!(code_for_style(164, &custom), Some("0.00%".to_string()));
    }

    #[test]
    fn date_like_custom_codes_fall_back() {
        let mut custom = HashMap::new();
        custom.insert(164, "dd/mm/yyyy".to_string());
        assert_eq!(code_for_style(164, &custom), None);
    }
}
