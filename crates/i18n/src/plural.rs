//! CLDR plural rules: the subset integer counts need (docs/catalogs.md section 5).

use std::collections::BTreeMap;

use serde_json::Value;

/// A language's plural rules: category (`zero`, `one`, `two`, `few`, `many`) to rule text.
/// `other` is the implicit remainder.
pub type PluralRules = BTreeMap<String, String>;

/// The categories a rule set may define, in evaluation order.
pub const CATEGORIES: [&str; 5] = ["zero", "one", "two", "few", "many"];

/// English: `one` for exactly 1.
pub fn english_rules() -> PluralRules {
    PluralRules::from([("one".to_string(), "i = 1 and v = 0".to_string())])
}

/// A rules object read from JSON; `None` when it is not an object of strings.
pub fn rules_from_value(value: &Value) -> Option<PluralRules> {
    value
        .as_object()?
        .iter()
        .map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Relation {
    /// The operand is `n` or `i` (the count); the others (`v w f t e c`) are 0 for integers.
    counts: bool,
    modulo: i128,
    equal: bool,
    spans: Vec<(i128, i128)>,
}

impl Relation {
    fn parse(text: &str) -> Option<Relation> {
        let mut chars = text.trim().chars();
        let operand = chars.next().filter(|c| "nivwftec".contains(*c))?;
        let mut rest = chars.as_str().trim_start();
        let mut modulo = 0;
        if let Some(r) = rest.strip_prefix('%') {
            let r = r.trim_start();
            let end = r.find(|c: char| !c.is_ascii_digit()).unwrap_or(r.len());
            if end == 0 {
                return None;
            }
            modulo = r[..end].parse().ok()?;
            rest = r[end..].trim_start();
        }
        let (equal, rest) = match rest.strip_prefix("!=") {
            Some(r) => (false, r),
            None => (true, rest.strip_prefix('=')?),
        };
        let ranges = rest.trim_start();
        if ranges.is_empty()
            || !ranges
                .chars()
                .all(|c| c.is_ascii_digit() || c == '.' || c == ',' || c.is_whitespace())
        {
            return None;
        }
        let mut spans = Vec::new();
        for item in ranges.split(',') {
            let (lo, hi) = item.trim().split_once("..").unwrap_or((item.trim(), ""));
            let lo: i128 = lo.trim().parse().ok()?;
            let hi: i128 = if hi.is_empty() {
                lo
            } else {
                hi.trim().parse().ok()?
            };
            spans.push((lo, hi));
        }
        Some(Relation {
            counts: "ni".contains(operand),
            modulo,
            equal,
            spans,
        })
    }

    fn holds(&self, n: i128) -> bool {
        let mut value = if self.counts { n } else { 0 };
        if self.modulo != 0 {
            value %= self.modulo;
        }
        self.spans
            .iter()
            .any(|&(lo, hi)| lo <= value && value <= hi)
            == self.equal
    }
}

type Rule = Vec<Vec<Relation>>;

/// One rule as `or` of `and` of relations: `None` when it does not parse, `Some(None)` for an
/// empty rule.
fn parse_rule(rule: &str) -> Option<Option<Rule>> {
    let body = rule.split('@').next().unwrap_or("");
    if body.trim().is_empty() {
        return Some(None);
    }
    body.split(" or ")
        .map(|conj| {
            conj.split(" and ")
                .map(Relation::parse)
                .collect::<Option<Vec<_>>>()
        })
        .collect::<Option<Rule>>()
        .map(Some)
}

/// Rules parsed once. A rule that does not parse is left out (its category is never chosen).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompiledRules(Vec<(&'static str, Rule)>);

/// Parses `rules` once.
pub fn compile_rules(rules: &PluralRules) -> CompiledRules {
    let mut out = Vec::new();
    for cat in CATEGORIES {
        if let Some(Some(Some(rule))) = rules.get(cat).map(|r| parse_rule(r)) {
            out.push((cat, rule));
        }
    }
    CompiledRules(out)
}

impl CompiledRules {
    /// The category of the integer count `n` (`other` when no rule holds); the sign is ignored.
    pub fn category(&self, n: i64) -> &'static str {
        let n = i128::from(n).abs();
        for (cat, conjs) in &self.0 {
            if conjs.iter().any(|conj| conj.iter().all(|r| r.holds(n))) {
                return cat;
            }
        }
        "other"
    }
}

/// The category of `n` under `rules` (parses them on each call; use [`compile_rules`] on a hot
/// path).
pub fn plural_category(rules: &PluralRules, n: i64) -> &'static str {
    compile_rules(rules).category(n)
}

/// True when every key is a CLDR category and every rule parses (the check for a language's
/// `numbers.json`).
pub fn valid_rules(rules: &PluralRules) -> bool {
    rules
        .iter()
        .all(|(cat, rule)| CATEGORIES.contains(&cat.as_str()) && parse_rule(rule).is_some())
}
