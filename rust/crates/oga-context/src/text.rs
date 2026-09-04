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

pub fn raw_words(text: &str) -> Vec<String> {
    let mut expanded = String::with_capacity(text.len() + 8);
    let chars = text.chars().collect::<Vec<_>>();
    for (index, char) in chars.iter().enumerate() {
        if index > 0 {
            let previous = chars[index - 1];
            let next_is_lower = chars.get(index + 1).is_some_and(|next| next.is_lowercase());
            if (previous.is_lowercase() || previous.is_ascii_digit()) && char.is_uppercase()
                || previous.is_uppercase() && char.is_uppercase() && next_is_lower
            {
                expanded.push(' ');
            }
        }
        expanded.push(char.to_ascii_lowercase());
    }
    expanded
        .split(|char: char| !char.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
        .collect()
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
    raw_words(text)
        .into_iter()
        .map(|word| normalize_word(&word))
        .collect()
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
            if whole.len() > 2 && !MAP_STOP_WORDS.contains(&whole.as_str()) {
                terms.insert(whole);
            }
        }
        for word in raw_words(token) {
            if word.len() > 2 && !MAP_STOP_WORDS.contains(&word.as_str()) {
                terms.insert(normalize_word(&word));
            }
        }
    }
    terms.into_iter().collect()
}

pub fn hint_key(hints: &[String]) -> String {
    let mut words = BTreeSet::new();
    for hint in hints {
        for word in raw_words(hint) {
            if word.len() > 2 && !FTS_STOP_WORDS.contains(&word.as_str()) {
                words.insert(normalize_word(&word));
            }
        }
    }
    words.into_iter().collect::<Vec<_>>().join(" ")
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
        tokens.insert(value.to_ascii_lowercase());
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
