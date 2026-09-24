use std::collections::BTreeSet;

pub const MAP_STOP_WORDS: &[&str] = &[
    "and", "are", "did", "does", "for", "from", "get", "got", "how", "into", "that", "the", "this",
    "was", "were", "what", "when", "where", "which", "who", "why", "with", "work",
];

pub const FTS_STOP_WORDS: &[&str] = &[
    "and", "are", "did", "does", "for", "from", "get", "got", "how", "into", "that", "the", "this",
    "was", "were", "what", "when", "where", "which", "who", "why", "with", "work", "a", "an", "be",
    "by", "is", "it", "of", "on", "or", "to", "use",
];

// Two letters can name a real symbol; these two-letter words are filler.
const SHORT_STOP_WORDS: &[&str] = &[
    "am", "an", "as", "at", "be", "by", "do", "he", "if", "in", "is", "it", "me", "my", "no", "of",
    "on", "or", "so", "to", "up", "us", "we",
];

fn searchable(word: &str, filler: &[&str]) -> bool {
    word.len() >= 2 && !SHORT_STOP_WORDS.contains(&word) && !filler.contains(&word)
}

#[derive(Debug)]
pub struct RawWords {
    expanded: String,
    ranges: Vec<(usize, usize)>,
}

impl RawWords {
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.ranges
            .iter()
            .map(|(start, end)| &self.expanded[*start..*end])
    }
}

pub fn raw_words(text: &str) -> RawWords {
    let mut expanded = String::with_capacity(text.len() + 8);
    let mut previous: Option<char> = None;
    let mut chars = text.chars().peekable();
    while let Some(char) = chars.next() {
        if let Some(previous) = previous {
            let next_is_lower = chars.peek().is_some_and(|next| next.is_lowercase());
            if (previous.is_lowercase() || previous.is_ascii_digit()) && char.is_uppercase()
                || previous.is_uppercase() && char.is_uppercase() && next_is_lower
            {
                expanded.push(' ');
            }
        }
        expanded.push(char.to_ascii_lowercase());
        previous = Some(char);
    }
    let ranges = expanded
        .split_inclusive(|char: char| !char.is_ascii_alphanumeric())
        .scan(0, |offset, part| {
            let start = *offset;
            *offset += part.len();
            let end = start
                + part
                    .trim_end_matches(|char: char| !char.is_ascii_alphanumeric())
                    .len();
            (!part[..end - start].is_empty()).then_some((start, end))
        })
        .collect();
    RawWords { expanded, ranges }
}

pub fn normalize_word(word: &str) -> String {
    if MAP_STOP_WORDS.contains(&word) {
        return word.to_owned();
    }
    let base = match word {
        "built" => "build",
        "chosen" => "choose",
        "reconnect" => "connect",
        "routing" => "route",
        "swept" => "sweep",
        "scored" => "score",
        "stored" => "store",
        "written" => "record",
        _ => word,
    };
    let mut normalized = base.to_owned();
    if normalized.len() > 6 && normalized.ends_with("ness") {
        normalized.truncate(normalized.len() - 4);
    } else if normalized.len() > 4 && (normalized.ends_with("ies") || normalized.ends_with("ied")) {
        normalized.truncate(normalized.len() - 3);
        normalized.push('y');
    } else if normalized.len() > 5 && normalized.ends_with("ing") {
        normalized.truncate(normalized.len() - 3);
        collapse_final_double(&mut normalized);
    } else if normalized.len() > 4 && normalized.ends_with("ed") {
        if ["ated", "bled", "uled", "ued", "ced", "ived", "ored", "oved"]
            .iter()
            .any(|suffix| normalized.ends_with(suffix))
        {
            normalized.truncate(normalized.len() - 1);
        } else {
            normalized.truncate(normalized.len() - 2);
            collapse_final_double(&mut normalized);
        }
    } else if normalized.len() > 3 && normalized.ends_with('s') && !normalized.ends_with("ss") {
        normalized.pop();
    }
    if normalized.len() > 3 && normalized.ends_with('e') {
        let before = normalized.as_bytes()[normalized.len() - 2] as char;
        if !"aeiou".contains(before) {
            normalized.pop();
        }
    }
    normalized
}

fn collapse_final_double(word: &mut String) {
    let bytes = word.as_bytes();
    if bytes.len() >= 2 && bytes[bytes.len() - 1] == bytes[bytes.len() - 2] {
        word.pop();
    }
}

pub fn words(text: &str) -> Vec<String> {
    raw_words(text).iter().map(normalize_word).collect()
}

pub fn name_key(text: &str) -> String {
    let all = words(text);
    let mut parts = all
        .iter()
        .filter(|word| !FTS_STOP_WORDS.contains(&word.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if parts.is_empty() {
        parts = all;
    }
    parts.sort();
    parts.dedup();
    parts.join(" ")
}

pub fn identifier_words(name: &str) -> Vec<String> {
    let mut values = vec![normalize_word(&name.to_ascii_lowercase())];
    values.extend(words(name));
    values.sort();
    values.dedup();
    values
}

pub fn prompt_terms(text: &str) -> Vec<String> {
    let mut terms = BTreeSet::new();
    for token in
        text.split(|char: char| !char.is_ascii_alphanumeric() && char != '_' && char != '-')
    {
        if token.is_empty() {
            continue;
        }
        if token.contains('_') || token.contains('-') || has_camel_boundary(token) {
            let whole = normalize_word(&token.to_ascii_lowercase());
            if searchable(&whole, MAP_STOP_WORDS) {
                terms.insert(whole);
            }
        }
        for word in raw_words(token).iter() {
            if searchable(word, MAP_STOP_WORDS) {
                terms.insert(normalize_word(word));
            }
        }
    }
    terms.into_iter().collect()
}

pub fn hint_words(text: &str) -> Vec<String> {
    raw_words(text)
        .iter()
        .map(normalize_word)
        .filter(|word| searchable(word, FTS_STOP_WORDS))
        .collect()
}

pub fn hint_key(hints: &[String]) -> String {
    hints
        .iter()
        .flat_map(|hint| hint_words(hint))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn fts_query(terms: &[String]) -> String {
    terms
        .iter()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ")
}

pub fn identifier_tokens(values: &[&str]) -> String {
    let mut tokens = BTreeSet::new();
    for value in values {
        tokens.extend(identifier_words(value));
    }
    tokens.into_iter().collect::<Vec<_>>().join(" ")
}

fn has_camel_boundary(value: &str) -> bool {
    let chars = value.chars().collect::<Vec<_>>();
    chars.windows(2).enumerate().any(|(index, pair)| {
        pair[0].is_lowercase() && pair[1].is_uppercase()
            || pair[0].is_uppercase()
                && pair[1].is_uppercase()
                && chars.get(index + 2).is_some_and(|char| char.is_lowercase())
    })
}

pub fn clean_comment(raw: &str) -> Option<String> {
    let block = raw.trim_start().starts_with("/*");
    let last = raw.lines().count().saturating_sub(1);
    let lines = raw.lines().enumerate().map(|(index, line)| {
        let mut line = line.trim().to_owned();
        if block && index == 0 {
            line = line.trim_start_matches("/*").trim_start().to_owned();
        }
        if block && index == last {
            line = line.trim_end_matches("*/").trim_end().to_owned();
        }
        if block {
            line = line.trim_start_matches('*').trim_start().to_owned();
        } else {
            line = line.trim_start_matches(['/', '#']).trim_start().to_owned();
        }
        line
    });
    let mut kept = Vec::new();
    for line in lines {
        if !is_noisy_comment(&line) {
            kept.push(line);
        }
    }
    while kept.first().is_some_and(String::is_empty) {
        kept.remove(0);
    }
    while kept.last().is_some_and(String::is_empty) {
        kept.pop();
    }
    (!kept.is_empty()).then(|| kept.join("\n").trim().to_owned())
}

fn is_noisy_comment(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    if line.is_empty()
        || lower.contains("spdx-license-identifier")
        || lower.contains("copyright")
        || lower.contains("licensed under")
        || lower.contains("todo")
        || lower.contains("fixme")
        || lower.contains("hack")
        || lower.contains("xxx")
    {
        return true;
    }
    if [
        "eslint-",
        "ts-",
        "@ts-",
        "prettier-",
        "istanbul",
        "coverage",
        "swiftlint",
        "clang-format",
        "noqa",
        "pragma",
        "region",
        "endregion",
        "sourcemappingurl",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
    {
        return true;
    }
    let starts_with_code = [
        "export ",
        "import ",
        "const ",
        "let ",
        "var ",
        "function ",
        "class ",
        "if ",
        "for ",
        "return ",
        "#include ",
        "def ",
        "func ",
        "struct ",
        "enum ",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix));
    if starts_with_code && line.chars().any(|char| "{}();=<>".contains(char)) {
        return true;
    }
    if !line.chars().any(|char| char.is_ascii_alphabetic()) {
        return true;
    }
    let non_whitespace = line
        .chars()
        .filter(|char| !char.is_whitespace())
        .collect::<Vec<_>>();
    let repeated = "=*#-_~"
        .chars()
        .map(|char| non_whitespace.iter().filter(|item| **item == char).count())
        .max()
        .unwrap_or_default();
    repeated * 5 >= non_whitespace.len() * 4
}
