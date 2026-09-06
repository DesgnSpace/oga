//! What JSON, TOML and YAML share. A config file has no declarations, so the
//! index treats every key as a symbol and its dotted path as the qualified
//! name. A key that holds a container is a `module`, a key that holds a scalar
//! is a `field`, and a container inside a sequence is known by its position.

use tree_sitter::Node;

use crate::symbols::{ExtractedSymbol, one_line};

/// The longest value that travels with its key. Long enough for a URL, a
/// version or a command name, short enough that no file's data drowns out its
/// keys.
const MAX_VALUE_BYTES: usize = 64;

/// Sequence containers, whose children have positions instead of names.
const SEQUENCES: &[&str] = &["array", "flow_sequence", "block_sequence"];

/// A generated dependency graph: thousands of keys, none of them written by
/// anyone, and no question is ever about one.
pub fn is_lockfile(path: &str) -> bool {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.ends_with(".lock") || file.contains("-lock.") || file == "Package.resolved"
}

/// Give a positional key its index and cut a long value out of the signature.
pub fn refine(symbol: &mut ExtractedSymbol, node: Node<'_>, source: &str) {
    if node.kind() == "table_array_element" {
        if let Some(index) = repeat_index(node, source) {
            symbol.name = format!("{}[{index}]", symbol.name);
            symbol.qualified = format!("{}[{index}]", symbol.qualified);
        }
        symbol.signature = signature(node, source);
        return;
    }
    match sequence_index(node) {
        Some(index) => {
            symbol.name = format!("[{index}]");
            symbol.qualified = match &symbol.parent {
                Some(parent) => format!("{parent}{}", symbol.name),
                None => symbol.name.clone(),
            };
            symbol.signature = String::new();
        }
        None => symbol.signature = signature(node, source),
    }
}

/// The position of a node inside the sequence that holds it.
fn sequence_index(node: Node<'_>) -> Option<usize> {
    let parent = node.parent()?;
    if !SEQUENCES.contains(&parent.kind()) {
        return None;
    }
    parent
        .named_children(&mut parent.walk())
        .position(|child| child.id() == node.id())
}

/// TOML writes repeated tables as `[[bin]]` siblings rather than as one array,
/// so their position is how many headers before this one carried the same key.
fn repeat_index(node: Node<'_>, source: &str) -> Option<usize> {
    let header = source.get(node.named_child(0)?.byte_range())?;
    let mut index = 0;
    let mut previous = node.prev_named_sibling();
    while let Some(candidate) = previous {
        if candidate.kind() == "table_array_element"
            && candidate
                .named_child(0)
                .and_then(|key| source.get(key.byte_range()))
                == Some(header)
        {
            index += 1;
        }
        previous = candidate.prev_named_sibling();
    }
    Some(index)
}

/// The key, plus its value when the value is short enough to travel with it.
fn signature(node: Node<'_>, source: &str) -> String {
    let end = value(node)
        .filter(|value| !travels_with_key(node, *value))
        .map_or(node.end_byte(), |value| value.start_byte());
    one_line(source.get(node.start_byte()..end).unwrap_or_default())
}

/// A value belongs in the signature when it is a short scalar beside its key.
/// Anything spilling onto another line is a container or a wall of data, and
/// neither is what someone is looking for.
fn travels_with_key(key: Node<'_>, value: Node<'_>) -> bool {
    value.start_position().row == key.start_position().row
        && value.byte_range().len() < MAX_VALUE_BYTES
}

/// What a key is set to. JSON and YAML name the field; TOML leaves the pair's
/// two children unnamed, so the value is whatever follows the key.
fn value(node: Node<'_>) -> Option<Node<'_>> {
    if let Some(value) = node.child_by_field_name("value") {
        return Some(value);
    }
    (node.kind() == "pair" && node.named_child_count() == 2).then(|| node.named_child(1))?
}
