use std::collections::BTreeSet;

use oga_domain::{ContextRefs, SourceLang, SymbolKind};

use crate::text::clean_comment;
use crate::walk::GenericLanguage;

#[derive(Debug, Clone)]
pub struct ExtractedSymbol {
    pub line: u64,
    pub end_line: u64,
    pub kind: SymbolKind,
    pub name: String,
    pub params: Option<String>,
    pub returns: Option<String>,
    pub exported: bool,
    pub purpose: Option<String>,
    pub comments: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ExtractedFile {
    pub symbols: Vec<ExtractedSymbol>,
    pub unparsed: bool,
    pub unparsed_reason: Option<String>,
    pub header_comment: Option<String>,
}

pub fn extract_symbols(
    source: &str,
    lang: SourceLang,
    generic: Option<GenericLanguage>,
) -> ExtractedFile {
    let lines = source.lines().collect::<Vec<_>>();
    if !matches!(lang, SourceLang::Generic)
        && let Some(reason) = balance_failure(source)
    {
        let reason = if matches!(lang, SourceLang::Ts) {
            "TypeScript syntax scan failed".to_owned()
        } else {
            reason
        };
        return ExtractedFile {
            unparsed: true,
            unparsed_reason: Some(reason),
            ..ExtractedFile::default()
        };
    }

    let mut symbols = match lang {
        SourceLang::Ts => scan_typescript(&lines),
        SourceLang::Swift => scan_swift(&lines),
        SourceLang::Generic => scan_generic(&lines, generic.unwrap_or(GenericLanguage::Unknown)),
    };
    for index in 0..symbols.len() {
        symbols[index].end_line = symbols
            .get(index + 1)
            .map_or(lines.len() as u64, |symbol| symbol.line.saturating_sub(1));
        let line = symbols[index].line.saturating_sub(1) as usize;
        let comments = preceding_comments(&lines, line);
        symbols[index].comments = comments.clone();
        symbols[index].purpose = comments
            .first()
            .map(|comment| {
                comment
                    .replace('\n', " ")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .filter(|purpose| !purpose.is_empty())
            .map(|purpose| purpose.chars().take(100).collect());
    }
    let first_symbol = symbols
        .first()
        .map_or(lines.len(), |symbol| symbol.line as usize - 1);
    let header = preceding_comments(&lines, first_symbol)
        .into_iter()
        .filter(|_| first_symbol > 0)
        .collect::<Vec<_>>();
    ExtractedFile {
        symbols,
        unparsed: false,
        unparsed_reason: None,
        header_comment: (!header.is_empty()).then(|| header.join("\n")),
    }
}

pub fn extract_refs(
    source: &str,
    lang: SourceLang,
    generic: Option<GenericLanguage>,
) -> ContextRefs {
    let mut imports = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim();
        match lang {
            SourceLang::Ts => {
                if let Some(value) = quoted_after(trimmed, "from")
                    .or_else(|| quoted_after(trimmed, "import"))
                    .or_else(|| quoted_after(trimmed, "require"))
                {
                    imports.push(value);
                }
            }
            SourceLang::Swift => {
                if let Some(value) = trimmed.strip_prefix("import ") {
                    imports.push(
                        value
                            .split_whitespace()
                            .next()
                            .unwrap_or_default()
                            .to_owned(),
                    );
                }
            }
            SourceLang::Generic => match generic.unwrap_or(GenericLanguage::Unknown) {
                GenericLanguage::Python => {
                    if let Some(value) = trimmed.strip_prefix("from ")
                        && value.starts_with('.')
                    {
                        imports.push(
                            value
                                .split_whitespace()
                                .next()
                                .unwrap_or_default()
                                .to_owned(),
                        );
                    } else if let Some(value) = trimmed.strip_prefix("import ")
                        && value.starts_with('.')
                    {
                        imports.push(
                            value
                                .split_whitespace()
                                .next()
                                .unwrap_or_default()
                                .to_owned(),
                        );
                    }
                }
                GenericLanguage::Rust => {
                    if let Some(value) = trimmed.strip_prefix("use ")
                        && (value.starts_with("crate::")
                            || value.starts_with("self::")
                            || value.starts_with("super::"))
                    {
                        imports.push(value.trim_end_matches(';').to_owned());
                    }
                }
                GenericLanguage::Php => {
                    for keyword in ["require", "require_once", "include", "include_once"] {
                        if let Some(value) = quoted_after(trimmed, keyword) {
                            imports.push(value);
                        }
                    }
                }
                GenericLanguage::Ruby => {
                    if let Some(value) = quoted_after(trimmed, "require_relative") {
                        imports.push(value);
                    }
                }
                GenericLanguage::C | GenericLanguage::Cpp => {
                    if let Some(value) = trimmed.strip_prefix("#include ")
                        && let Some(value) = value
                            .strip_prefix('"')
                            .and_then(|value| value.split('"').next())
                    {
                        imports.push(value.to_owned());
                    }
                }
                _ => {}
            },
        }
    }
    imports = dedupe(imports, 30);
    ContextRefs {
        imports,
        calls: call_sites(source),
    }
}

fn scan_typescript(lines: &[&str]) -> Vec<ExtractedSymbol> {
    let mut symbols = Vec::new();
    let mut depth = 0_i32;
    let deltas = brace_deltas(lines);
    for (index, line) in lines.iter().enumerate() {
        if depth == 0 {
            let stripped = strip_ts_prefixes(line.trim());
            if !is_skipped_typescript_declaration(&stripped)
                && let Some((kind, name)) = declaration_name(&stripped, SourceLang::Ts, None)
            {
                let text = declaration_text(lines, index, kind != SymbolKind::Const);
                let (params, returns) = signature(&text, SourceLang::Ts);
                let is_arrow = kind == SymbolKind::Const && has_top_level_arrow(&text);
                let (params, returns) = if kind == SymbolKind::Const && !is_arrow {
                    (None, None)
                } else {
                    (params, returns)
                };
                if !(is_arrow && text.lines().count() < 3) {
                    symbols.push(ExtractedSymbol {
                        line: index as u64 + 1,
                        end_line: index as u64 + 1,
                        kind,
                        name,
                        params,
                        returns,
                        exported: line.trim_start().starts_with("export"),
                        purpose: None,
                        comments: Vec::new(),
                    });
                }
            }
        }
        depth += deltas[index];
        depth = depth.max(0);
    }
    symbols
}

fn is_skipped_typescript_declaration(line: &str) -> bool {
    line.starts_with("enum ")
        || line.starts_with("const enum ")
        || line.starts_with("namespace ")
        || line.starts_with("interface ")
        || line.starts_with("import ")
        || line.starts_with('{')
        || line.starts_with('*')
        || line.starts_with("type {")
}

fn has_top_level_arrow(text: &str) -> bool {
    let mut braces = 0_i32;
    let chars = text.chars().collect::<Vec<_>>();
    for (index, char) in chars.iter().enumerate() {
        match char {
            '{' => braces += 1,
            '}' => braces = (braces - 1).max(0),
            '=' if braces == 0 && chars.get(index + 1) == Some(&'>') => return true,
            _ => {}
        }
    }
    false
}

fn scan_swift(lines: &[&str]) -> Vec<ExtractedSymbol> {
    let mut symbols = Vec::new();
    let mut depth = 0_i32;
    let deltas = brace_deltas(lines);
    for (index, line) in lines.iter().enumerate() {
        if depth == 0 {
            let stripped = strip_swift_prefixes(line.trim());
            if let Some((mut kind, name)) = declaration_name(&stripped, SourceLang::Swift, None) {
                let text = declaration_text(lines, index, kind != SymbolKind::Const);
                let (params, returns) = signature(&text, SourceLang::Swift);
                if kind == SymbolKind::Struct
                    && text
                        .split('{')
                        .next()
                        .is_some_and(|head| head.contains("View"))
                {
                    kind = SymbolKind::View;
                }
                symbols.push(ExtractedSymbol {
                    line: index as u64 + 1,
                    end_line: index as u64 + 1,
                    kind,
                    name,
                    params,
                    returns,
                    exported: line.trim_start().starts_with("public ")
                        || line.trim_start().starts_with("open "),
                    purpose: None,
                    comments: Vec::new(),
                });
            }
        }
        depth += deltas[index];
        depth = depth.max(0);
    }
    symbols
}

fn scan_generic(lines: &[&str], language: GenericLanguage) -> Vec<ExtractedSymbol> {
    let indent_based = matches!(language, GenericLanguage::Python | GenericLanguage::Ruby);
    if indent_based {
        return scan_generic_indent(lines, language);
    }
    let mut symbols = Vec::new();
    let mut depth = 0_i32;
    let mut container_depth = None;
    let deltas = brace_deltas(lines);
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let eligible = depth == 0 || container_depth == Some(depth);
        if eligible
            && let Some((kind, name)) =
                declaration_name(trimmed, SourceLang::Generic, Some(language))
        {
            let text = declaration_text(lines, index, kind != SymbolKind::Const);
            let (params, returns) = signature(&text, SourceLang::Generic);
            let exported = generic_exported(trimmed, &name, language);
            symbols.push(ExtractedSymbol {
                line: index as u64 + 1,
                end_line: index as u64 + 1,
                kind,
                name,
                params,
                returns,
                exported,
                purpose: None,
                comments: Vec::new(),
            });
            if depth == 0
                && matches!(
                    kind,
                    SymbolKind::Class | SymbolKind::Struct | SymbolKind::Enum | SymbolKind::Ext
                )
            {
                container_depth = Some(depth + 1);
            }
        }
        depth += deltas[index];
        depth = depth.max(0);
        if container_depth.is_some_and(|container| depth < container) {
            container_depth = None;
        }
    }
    symbols
}

fn scan_generic_indent(lines: &[&str], language: GenericLanguage) -> Vec<ExtractedSymbol> {
    let mut symbols = Vec::new();
    let mut stack = Vec::<(usize, bool)>::new();
    for (index, raw) in lines.iter().enumerate() {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = raw.len() - raw.trim_start().len();
        while stack.last().is_some_and(|(level, _)| indent <= *level) {
            stack.pop();
        }
        if (stack.is_empty()
            || stack
                .first()
                .is_some_and(|(_, container)| *container && stack.len() == 1))
            && let Some((kind, name)) =
                declaration_name(trimmed, SourceLang::Generic, Some(language))
        {
            symbols.push(ExtractedSymbol {
                line: index as u64 + 1,
                end_line: index as u64 + 1,
                kind,
                name: name.clone(),
                params: signature(&declaration_text(lines, index, false), SourceLang::Generic).0,
                returns: signature(&declaration_text(lines, index, false), SourceLang::Generic).1,
                exported: generic_exported(trimmed, &name, language),
                purpose: None,
                comments: Vec::new(),
            });
        }
        if trimmed.ends_with(':')
            || trimmed.contains(':') && matches!(language, GenericLanguage::Python)
        {
            stack.push((indent, trimmed.starts_with("class ")));
        }
    }
    symbols
}

fn declaration_name(
    line: &str,
    lang: SourceLang,
    generic: Option<GenericLanguage>,
) -> Option<(SymbolKind, String)> {
    match lang {
        SourceLang::Ts => {
            for (prefix, kind) in [
                ("function ", SymbolKind::Fn),
                ("class ", SymbolKind::Class),
                ("type ", SymbolKind::Type),
                ("const ", SymbolKind::Const),
            ] {
                if let Some(name) = identifier_after(line, prefix) {
                    return Some((kind, name));
                }
            }
        }
        SourceLang::Swift => {
            for (prefix, kind) in [
                ("func ", SymbolKind::Fn),
                ("struct ", SymbolKind::Struct),
                ("class ", SymbolKind::Class),
                ("enum ", SymbolKind::Enum),
                ("extension ", SymbolKind::Ext),
            ] {
                if let Some(name) = identifier_after(line, prefix) {
                    return Some((kind, name));
                }
            }
        }
        SourceLang::Generic => {
            let language = generic.unwrap_or(GenericLanguage::Unknown);
            let rules: &[(&str, SymbolKind)] = match language {
                GenericLanguage::Python => &[
                    ("async def ", SymbolKind::Fn),
                    ("def ", SymbolKind::Fn),
                    ("class ", SymbolKind::Class),
                ],
                GenericLanguage::Go => &[("func ", SymbolKind::Fn), ("type ", SymbolKind::Type)],
                GenericLanguage::Rust => &[
                    ("fn ", SymbolKind::Fn),
                    ("struct ", SymbolKind::Struct),
                    ("enum ", SymbolKind::Enum),
                    ("trait ", SymbolKind::Type),
                    ("impl ", SymbolKind::Ext),
                ],
                GenericLanguage::Java => &[
                    ("class ", SymbolKind::Class),
                    ("interface ", SymbolKind::Type),
                    ("enum ", SymbolKind::Enum),
                ],
                GenericLanguage::Kotlin => &[
                    ("class ", SymbolKind::Class),
                    ("interface ", SymbolKind::Type),
                    ("object ", SymbolKind::Class),
                    ("fun ", SymbolKind::Fn),
                ],
                GenericLanguage::Php => &[
                    ("class ", SymbolKind::Class),
                    ("interface ", SymbolKind::Type),
                    ("trait ", SymbolKind::Type),
                    ("function ", SymbolKind::Fn),
                ],
                GenericLanguage::Ruby => &[
                    ("class ", SymbolKind::Class),
                    ("module ", SymbolKind::Type),
                    ("def ", SymbolKind::Fn),
                ],
                GenericLanguage::C | GenericLanguage::Cpp => &[
                    ("struct ", SymbolKind::Struct),
                    ("class ", SymbolKind::Class),
                    ("enum ", SymbolKind::Enum),
                ],
                GenericLanguage::Csharp => &[
                    ("class ", SymbolKind::Class),
                    ("interface ", SymbolKind::Type),
                    ("enum ", SymbolKind::Enum),
                    ("namespace ", SymbolKind::Ext),
                ],
                GenericLanguage::Unknown => &[
                    ("function ", SymbolKind::Fn),
                    ("fn ", SymbolKind::Fn),
                    ("class ", SymbolKind::Class),
                    ("struct ", SymbolKind::Struct),
                    ("enum ", SymbolKind::Enum),
                    ("interface ", SymbolKind::Type),
                ],
            };
            let normalized = strip_generic_modifiers(line, language);
            for (prefix, kind) in rules {
                if let Some(name) = identifier_after(&normalized, prefix) {
                    return Some((*kind, name));
                }
            }
            if matches!(language, GenericLanguage::Go) && normalized.starts_with("type ") {
                return identifier_after(&normalized, "type ").map(|name| (SymbolKind::Type, name));
            }
            if matches!(language, GenericLanguage::Go)
                && normalized.starts_with("func (")
                && let Some(name) = normalized
                    .split(')')
                    .nth(1)
                    .and_then(|rest| rest.split('(').next())
                    .map(str::trim)
                    .filter(|name| is_identifier(name))
            {
                return Some((SymbolKind::Fn, name.to_owned()));
            }
            if matches!(language, GenericLanguage::C | GenericLanguage::Cpp)
                && let Some(name) = function_name(normalized.as_str())
            {
                return Some((SymbolKind::Fn, name));
            }
            if matches!(
                language,
                GenericLanguage::Java
                    | GenericLanguage::Kotlin
                    | GenericLanguage::Csharp
                    | GenericLanguage::Php
            ) && normalized.contains('(')
                && !normalized.ends_with(';')
                && let Some(name) = normalized
                    .split('(')
                    .next()
                    .and_then(|head| head.split_whitespace().last())
                && is_identifier(name)
            {
                return Some((SymbolKind::Fn, name.to_owned()));
            }
        }
    }
    None
}

fn strip_ts_prefixes(mut line: &str) -> String {
    loop {
        let next = ["export ", "declare ", "async ", "default ", "abstract "]
            .iter()
            .find_map(|prefix| line.strip_prefix(prefix));
        let Some(next) = next else {
            return line.to_owned();
        };
        line = next;
    }
}

fn strip_swift_prefixes(line: &str) -> String {
    let mut line = line.trim();
    while line.starts_with('@') {
        line = line
            .split_once(' ')
            .map_or("", |(_, rest)| rest.trim_start());
    }
    while let Some((first, rest)) = line.split_once(' ') {
        if [
            "public",
            "open",
            "internal",
            "fileprivate",
            "private",
            "final",
            "nonisolated",
            "convenience",
            "override",
            "required",
            "indirect",
        ]
        .contains(&first)
        {
            line = rest.trim_start();
        } else {
            break;
        }
    }
    line.to_owned()
}

fn strip_generic_modifiers(line: &str, language: GenericLanguage) -> String {
    let mut line = line.trim().to_owned();
    let modifiers: &[&str] = match language {
        GenericLanguage::Rust => &["pub", "async", "unsafe", "const"],
        GenericLanguage::Java
        | GenericLanguage::Kotlin
        | GenericLanguage::Csharp
        | GenericLanguage::Php => &[
            "public",
            "private",
            "protected",
            "internal",
            "static",
            "final",
            "abstract",
            "virtual",
            "override",
            "sealed",
            "partial",
            "data",
            "open",
            "suspend",
            "async",
            "extern",
            "inline",
            "const",
            "typedef",
            "using",
        ],
        _ => &[],
    };
    while let Some((first, rest)) = line.split_once(' ') {
        let bare = first.split('(').next().unwrap_or(first);
        if modifiers.contains(&bare) {
            line = rest.trim_start().to_owned();
        } else {
            break;
        }
    }
    line
}

fn generic_exported(line: &str, name: &str, language: GenericLanguage) -> bool {
    match language {
        GenericLanguage::Python | GenericLanguage::Ruby => !name.starts_with('_'),
        GenericLanguage::Go => name.chars().next().is_some_and(char::is_uppercase),
        GenericLanguage::Rust => {
            line.starts_with("pub ") || line.starts_with("pub(") || line.starts_with("pub(crate)")
        }
        GenericLanguage::Java | GenericLanguage::Csharp => line.contains("public"),
        GenericLanguage::Kotlin => !line.contains("private"),
        _ => true,
    }
}

fn identifier_after(line: &str, prefix: &str) -> Option<String> {
    let rest = line.strip_prefix(prefix)?.trim_start();
    let name = rest
        .chars()
        .take_while(|char| char.is_ascii_alphanumeric() || *char == '_' || *char == '$')
        .collect::<String>();
    is_identifier(&name).then_some(name)
}

fn function_name(line: &str) -> Option<String> {
    let before = line.split('(').next()?.trim();
    let name = before.split_whitespace().last()?;
    is_identifier(name).then_some(name.to_owned())
}

fn is_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.chars().enumerate().all(|(index, char)| {
            char.is_ascii_alphabetic()
                || char == '_'
                || char == '$'
                || (index > 0 && char.is_ascii_digit())
        })
}

fn declaration_text(lines: &[&str], start: usize, stops_at_body: bool) -> String {
    let mut text = String::new();
    let mut parentheses = 0_i32;
    let mut braces = 0_i32;
    for (index, line) in lines.iter().enumerate().skip(start) {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(line);
        let mut in_string = None;
        let chars = line.chars().collect::<Vec<_>>();
        for (at, char) in chars.iter().enumerate() {
            if in_string.is_some() {
                if in_string == Some(*char) && chars.get(at.wrapping_sub(1)) != Some(&'\\') {
                    in_string = None;
                }
                continue;
            }
            if *char == '"' || *char == '\'' || *char == '`' {
                in_string = Some(*char);
            } else if *char == '(' {
                parentheses += 1;
            } else if *char == ')' {
                parentheses = (parentheses - 1).max(0);
            } else if parentheses == 0 && *char == '{' && stops_at_body {
                return text;
            } else if *char == '{' {
                braces += 1;
            } else if *char == '}' {
                braces = (braces - 1).max(0);
            } else if parentheses == 0 && braces == 0 && *char == ';' {
                return text;
            }
        }
        if index > start + 12 {
            break;
        }
    }
    text
}

fn signature(text: &str, lang: SourceLang) -> (Option<String>, Option<String>) {
    let Some(open) = text.find('(') else {
        return (None, None);
    };
    let Some(close) = matching_delimiter(text, open, '(', ')') else {
        return (None, None);
    };
    let mut params = text[open + 1..close]
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| {
            if matches!(lang, SourceLang::Swift) {
                part.split(':')
                    .next()
                    .and_then(|prefix| prefix.split_whitespace().last())
                    .unwrap_or_default()
                    .to_owned()
            } else {
                part.split([':', '=', ' '])
                    .next()
                    .unwrap_or_default()
                    .to_owned()
            }
        })
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if params.len() > 4 {
        params.truncate(3);
        params.push("…".to_owned());
    }
    let params = (!params.is_empty()).then(|| params.join(", "));
    let after = text[close + 1..].trim();
    let returns = after
        .strip_prefix(':')
        .or_else(|| after.strip_prefix("->"))
        .map(|value| {
            value
                .split("=>")
                .next()
                .unwrap_or(value)
                .split('{')
                .next()
                .unwrap_or(value)
                .trim()
                .to_owned()
        })
        .filter(|value| !value.is_empty());
    (params, returns)
}

fn matching_delimiter(text: &str, open: usize, opening: char, closing: char) -> Option<usize> {
    let mut depth = 0_i32;
    for (index, char) in text.char_indices().skip_while(|(index, _)| *index < open) {
        if char == opening {
            depth += 1;
        } else if char == closing {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

fn preceding_comments(lines: &[&str], declaration: usize) -> Vec<String> {
    if declaration == 0 {
        return Vec::new();
    }
    let mut end = declaration;
    if lines
        .get(end.saturating_sub(1))
        .is_some_and(|line| line.trim().is_empty())
    {
        return Vec::new();
    }
    let mut blocks = Vec::new();
    while end > 0 {
        let trimmed = lines[end - 1].trim();
        if trimmed.starts_with("//") || trimmed.starts_with("#") {
            blocks.push(trimmed.to_owned());
            end -= 1;
            continue;
        }
        if trimmed.ends_with("*/") {
            let mut start = end - 1;
            while start > 0 && !lines[start].contains("/*") {
                start -= 1;
            }
            blocks.push(lines[start..end].join("\n"));
            end = start;
            continue;
        }
        break;
    }
    blocks.reverse();
    blocks
        .into_iter()
        .filter_map(|block| clean_comment(&block))
        .collect()
}

fn quoted_after(line: &str, keyword: &str) -> Option<String> {
    let at = line.find(keyword)?;
    let rest = &line[at + keyword.len()..];
    let quote = rest.find(['"', '\''])?;
    let delimiter = rest.as_bytes()[quote] as char;
    let value = &rest[quote + 1..];
    Some(value.split(delimiter).next().unwrap_or_default().to_owned())
}

fn call_sites(source: &str) -> Vec<String> {
    let stopwords = BTreeSet::from([
        "if",
        "for",
        "while",
        "switch",
        "catch",
        "return",
        "typeof",
        "new",
        "in",
        "of",
        "do",
        "else",
        "try",
        "await",
        "yield",
        "delete",
        "void",
        "instanceof",
        "case",
        "super",
        "this",
        "self",
        "constructor",
        "function",
        "func",
        "fn",
        "def",
        "class",
        "struct",
        "enum",
        "interface",
        "impl",
        "trait",
        "pub",
        "match",
        "loop",
        "unsafe",
        "where",
        "namespace",
        "public",
        "private",
        "protected",
        "static",
        "final",
        "abstract",
        "require",
        "include",
        "sizeof",
        "typedef",
        "template",
        "fun",
        "val",
        "var",
    ]);
    let mut values = Vec::new();
    let chars = source.chars().collect::<Vec<_>>();
    for index in 0..chars.len() {
        if chars[index] != '(' {
            continue;
        }
        let mut start = index;
        while start > 0
            && (chars[start - 1].is_ascii_alphanumeric()
                || chars[start - 1] == '_'
                || chars[start - 1] == '$')
        {
            start -= 1;
        }
        let name = chars[start..index].iter().collect::<String>();
        if is_identifier(&name) && !stopwords.contains(name.as_str()) {
            values.push(name);
        }
        if values.len() >= 200 {
            break;
        }
    }
    dedupe(values, 200)
}

fn dedupe(values: Vec<String>, limit: usize) -> Vec<String> {
    let mut seen = BTreeSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(value.clone()))
        .take(limit)
        .collect()
}

fn brace_deltas(lines: &[&str]) -> Vec<i32> {
    let mut block_comment = false;
    let mut quote = None;
    let mut escaped = false;
    let mut deltas = Vec::with_capacity(lines.len());
    for line in lines {
        let chars = line.chars().collect::<Vec<_>>();
        let mut delta = 0;
        let mut index = 0;
        while index < chars.len() {
            let char = chars[index];
            if block_comment {
                if char == '*' && chars.get(index + 1) == Some(&'/') {
                    block_comment = false;
                    index += 2;
                    continue;
                }
                index += 1;
                continue;
            }
            if let Some(delimiter) = quote {
                if char == delimiter && !escaped {
                    quote = None;
                }
                escaped = char == '\\' && !escaped;
                if char != '\\' {
                    escaped = false;
                }
                index += 1;
                continue;
            }
            if char == '/' && chars.get(index + 1) == Some(&'/') {
                break;
            }
            if char == '/' && chars.get(index + 1) == Some(&'*') {
                block_comment = true;
                index += 2;
                continue;
            }
            if char == '"' || char == '\'' || char == '`' {
                quote = Some(char);
                escaped = false;
            } else if char == '{' {
                delta += 1;
            } else if char == '}' {
                delta -= 1;
            }
            index += 1;
        }
        deltas.push(delta);
    }
    deltas
}

fn balance_failure(source: &str) -> Option<String> {
    let mut depth = 0_i32;
    let mut block_comment = false;
    let mut quote = None;
    let mut last_balanced = 0;
    let mut open_line = None;
    for (line_number, line) in source.lines().enumerate() {
        let chars = line.chars().collect::<Vec<_>>();
        let mut index = 0;
        while index < chars.len() {
            let char = chars[index];
            if block_comment {
                if char == '*' && chars.get(index + 1) == Some(&'/') {
                    block_comment = false;
                    index += 2;
                    continue;
                }
                index += 1;
                continue;
            }
            if quote.is_some() {
                if quote == Some(char) && (index == 0 || chars[index - 1] != '\\') {
                    quote = None;
                }
                index += 1;
                continue;
            }
            if char == '/' && chars.get(index + 1) == Some(&'/') {
                break;
            }
            if char == '/' && chars.get(index + 1) == Some(&'*') {
                block_comment = true;
                index += 2;
                continue;
            }
            if char == '"' || char == '\'' || char == '`' {
                quote = Some(char);
            } else if char == '{' {
                depth += 1;
                open_line.get_or_insert(line_number + 1);
            } else if char == '}' {
                depth -= 1;
                if depth < 0 {
                    return Some(format!("unbalanced braces (-1) after line {}", line_number));
                }
                if depth == 0 {
                    open_line = None;
                }
            }
            index += 1;
        }
        if depth == 0 && quote.is_none() && !block_comment {
            last_balanced = line_number + 1;
        }
    }
    if block_comment {
        return Some("unterminated block comment".into());
    }
    if quote.is_some() {
        return Some("unterminated string literal".into());
    }
    (depth != 0).then(|| {
        format!(
            "unbalanced braces (+{depth}) after line {last_balanced}; open at {}",
            open_line.unwrap_or_default()
        )
    })
}
