use crate::model::Block;

/// The marker family a list level uses in the source document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerKind {
    /// Unordered.
    Bullet,
    /// `1`, `2`, `3`.
    Decimal,
    /// `a`, `b`, `c`.
    LowerAlpha,
    /// `A`, `B`, `C`.
    UpperAlpha,
    /// `i`, `ii`, `iii`.
    LowerRoman,
    /// `I`, `II`, `III`.
    UpperRoman,
    /// Fork (sznb): CJK counting — `一`, `二`, `三` (numFmt chineseCounting,
    /// ideographDigital, chineseCountingThousand, ...). Ubiquitous in Chinese
    /// official documents; without it these levels degrade to `1`, `2`, `3`.
    ChineseCounting,
    /// Fork (sznb): CJK legal — `壹`, `贰`, `叁` (numFmt chineseLegalSimplified,
    /// ideographLegalTraditional, japaneseLegal).
    ChineseLegal,
}

impl MarkerKind {
    /// True for every kind but [`MarkerKind::Bullet`].
    pub fn ordered(self) -> bool {
        !matches!(self, MarkerKind::Bullet)
    }

    /// The marker text for ordinal `n` (1-based), without trailing space:
    /// `3.`, `c.`, `iv.`; CJK counting defaults to the ideographic comma
    /// (`一、`) — the punctuation official-document lists actually use.
    pub fn label(self, n: u64) -> String {
        match self {
            MarkerKind::Bullet => "-".to_string(),
            MarkerKind::ChineseCounting | MarkerKind::ChineseLegal => {
                format!("{}、", self.ordinal(n))
            }
            _ => format!("{}.", self.ordinal(n)),
        }
    }

    /// The bare ordinal text for `n` without punctuation: `3`, `c`, `iv`.
    pub fn ordinal(self, n: u64) -> String {
        match self {
            MarkerKind::Bullet => "-".to_string(),
            MarkerKind::Decimal => n.to_string(),
            MarkerKind::LowerAlpha => alpha(n),
            MarkerKind::UpperAlpha => alpha(n).to_ascii_uppercase(),
            MarkerKind::LowerRoman => roman(n),
            MarkerKind::UpperRoman => roman(n).to_ascii_uppercase(),
            MarkerKind::ChineseCounting => cjk_numeral(n, false),
            MarkerKind::ChineseLegal => cjk_numeral(n, true),
        }
    }
}

/// 1 -> `a`, 26 -> `z`, 27 -> `aa` (bijective base 26).
fn alpha(mut n: u64) -> String {
    if n == 0 {
        return "0".to_string();
    }
    let mut out = Vec::new();
    while n > 0 {
        n -= 1;
        out.push(b'a' + (n % 26) as u8);
        n /= 26;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

fn roman(mut n: u64) -> String {
    if n == 0 || n > 3999 {
        return n.to_string();
    }
    const NUMERALS: [(u64, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut out = String::new();
    for (value, numeral) in NUMERALS {
        while n >= value {
            out.push_str(numeral);
            n -= value;
        }
    }
    out
}

/// Fork (sznb): CJK numeral for chineseCounting-family markers.
///
/// Positional style with 万 grouping (`一万零一`, `十万零一千`), runs of
/// interior zeros collapsed to a single 零, and 10-19 reading `十` without a
/// leading 一 in plain style (Word's own rendering); the legal style keeps
/// the explicit 壹 (`壹拾`) as finance convention requires. Values beyond
/// 9999 9999 fall back to Arabic digits — list ordinals never reach them.
pub(crate) fn cjk_numeral(n: u64, legal: bool) -> String {
    if n == 0 {
        return "零".to_string();
    }
    if n > 99_999_999 {
        return n.to_string();
    }
    let digits: [&str; 10] = if legal {
        ["", "壹", "贰", "叁", "肆", "伍", "陆", "柒", "捌", "玖"]
    } else {
        ["", "一", "二", "三", "四", "五", "六", "七", "八", "九"]
    };
    // One 1..=9999 group: positional digits with units, interior zeros
    // collapsed, leading one before 十 dropped in plain style.
    let group = |g: u64| -> String {
        const UNITS: [&str; 4] = ["千", "百", "十", ""];
        let mut d = [0u64; 4];
        let mut v = g;
        for i in (0..4).rev() {
            d[i] = v % 10;
            v /= 10;
        }
        let mut out = String::new();
        let mut started = false;
        let mut pending_zero = false;
        for i in 0..4 {
            if d[i] == 0 {
                if started {
                    pending_zero = true;
                }
                continue;
            }
            if pending_zero {
                out.push('零');
                pending_zero = false;
            }
            let leading_one = d[i] == 1 && UNITS[i] == "十" && !started && !legal;
            if !leading_one {
                out.push_str(digits[d[i] as usize]);
            }
            out.push_str(UNITS[i]);
            started = true;
        }
        out
    };
    let wan = n / 10_000;
    let rest = n % 10_000;
    if wan == 0 {
        return group(n);
    }
    let mut out = group(wan);
    out.push_str("万");
    if rest > 0 {
        // A zero at the boundary (the 万 group's ones digit, or the ones
        // group's thousands digit) reads as a single 零: 一万零一 (10001),
        // 十万零一千 (101000); 一万一千 (11000) has none.
        if wan % 10 == 0 || rest < 1000 {
            out.push('零');
        }
        out.push_str(&group(rest));
    }
    out
}

/// A fully resolved list: numbering identity and marker resolution happen in
/// the frontends (`shared::list`), which split runs whenever the list
/// instance or marker kind changes.
#[derive(Debug, Clone)]
pub struct List {
    /// The marker family every item in this run uses.
    pub marker: MarkerKind,
    /// Ordinal of the first item, from the source's own numbering.
    pub start: u64,
    /// The items, in order.
    pub items: Vec<ListItem>,
}

impl List {
    /// True when this list's marker is a numbered one.
    pub fn ordered(&self) -> bool {
        self.marker.ordered()
    }
}

/// One item of a [`List`], which may hold nested blocks including further
/// lists.
#[derive(Debug, Clone, Default)]
pub struct ListItem {
    /// The item's content.
    pub blocks: Vec<Block>,
    /// Literal marker text that overrides the level marker when the source
    /// number text cannot be reproduced from `marker` + position alone
    /// (composite number text such as `1-a)`).
    pub marker_label: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_cover_the_marker_kinds() {
        assert_eq!(MarkerKind::Decimal.label(7), "7.");
        assert_eq!(MarkerKind::LowerAlpha.label(3), "c.");
        assert_eq!(MarkerKind::LowerAlpha.label(27), "aa.");
        assert_eq!(MarkerKind::UpperAlpha.label(2), "B.");
        assert_eq!(MarkerKind::LowerRoman.label(4), "iv.");
        assert_eq!(MarkerKind::LowerRoman.label(1994), "mcmxciv.");
        assert_eq!(MarkerKind::UpperRoman.label(9), "IX.");
        assert_eq!(MarkerKind::ChineseCounting.label(1), "一、");
        assert_eq!(MarkerKind::ChineseCounting.label(10), "十、");
        assert_eq!(MarkerKind::ChineseLegal.label(11), "壹拾壹、");
    }

    #[test]
    fn cjk_numerals_read_like_word() {
        assert_eq!(cjk_numeral(1, false), "一");
        assert_eq!(cjk_numeral(9, false), "九");
        assert_eq!(cjk_numeral(10, false), "十");
        assert_eq!(cjk_numeral(11, false), "十一");
        assert_eq!(cjk_numeral(20, false), "二十");
        assert_eq!(cjk_numeral(21, false), "二十一");
        assert_eq!(cjk_numeral(100, false), "一百");
        assert_eq!(cjk_numeral(101, false), "一百零一");
        assert_eq!(cjk_numeral(110, false), "一百一十");
        assert_eq!(cjk_numeral(120, false), "一百二十");
        assert_eq!(cjk_numeral(1000, false), "一千");
        assert_eq!(cjk_numeral(1001, false), "一千零一");
        assert_eq!(cjk_numeral(1100, false), "一千一百");
        assert_eq!(cjk_numeral(10000, false), "一万");
        assert_eq!(cjk_numeral(10001, false), "一万零一");
        assert_eq!(cjk_numeral(11000, false), "一万一千");
        assert_eq!(cjk_numeral(101000, false), "十万零一千");
        assert_eq!(cjk_numeral(12345, false), "一万二千三百四十五");
        assert_eq!(cjk_numeral(10, true), "壹拾");
        assert_eq!(cjk_numeral(11, true), "壹拾壹");
        assert_eq!(cjk_numeral(110, true), "壹佰壹拾");
    }
}
