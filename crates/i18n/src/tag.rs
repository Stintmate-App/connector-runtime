//! Language tags and the fallback chain (connector kit, catalogs.md section 6).

fn title(part: &str) -> String {
    let mut chars = part.chars();
    match chars.next() {
        Some(c) => c
            .to_uppercase()
            .chain(chars.flat_map(char::to_lowercase))
            .collect(),
        None => String::new(),
    }
}

fn in_subset(tag: &str) -> bool {
    let parts: Vec<&str> = tag.split('-').collect();
    let lower = |s: &str| s.bytes().all(|b| b.is_ascii_lowercase());
    let upper = |s: &str| s.bytes().all(|b| b.is_ascii_uppercase());
    if !(2..=3).contains(&parts[0].len()) || !lower(parts[0]) {
        return false;
    }
    let mut i = 1;
    if let Some(p) = parts.get(i)
        && p.len() == 4
        && p.is_ascii()
        && upper(&p[..1])
        && lower(&p[1..])
    {
        i += 1;
    }
    if let Some(p) = parts.get(i)
        && ((p.len() == 2 && upper(p)) || (p.len() == 3 && p.bytes().all(|b| b.is_ascii_digit())))
    {
        i += 1;
    }
    i == parts.len()
}

/// `es-cl` becomes `es-CL`; `None` when it is not a tag of the contract's subset
/// (language, optional script, optional region).
pub fn normalize_tag(tag: &str) -> Option<String> {
    let mut parts = tag.split('-');
    let mut out = vec![parts.next()?.to_lowercase()];
    for p in parts {
        out.push(if p.chars().count() == 4 {
            title(p)
        } else {
            p.to_uppercase()
        });
    }
    let norm = out.join("-");
    in_subset(&norm).then_some(norm)
}

/// The pack folders to try for `tag`, in order (the built-in English comes after them). An
/// invalid tag gives `["en"]`.
pub fn chain(tag: &str) -> Vec<String> {
    let norm = normalize_tag(tag).unwrap_or_else(|| "en".to_string());
    let parts: Vec<&str> = norm.split('-').collect();
    let mut out: Vec<String> = (1..=parts.len())
        .rev()
        .map(|i| parts[..i].join("-"))
        .collect();
    if !out.iter().any(|t| t == "en") {
        out.push("en".to_string());
    }
    out
}
