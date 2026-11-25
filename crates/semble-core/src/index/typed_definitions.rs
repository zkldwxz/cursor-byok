//! Extracts typed definitions, enum members and scoped declarations from the file's syntax tree.
use tree_sitter::{Node, Tree};

use super::snapshot::{IndexedDeclaration, LineSpan};
use super::IndexedDefinition;
use crate::Chunk;

pub(super) fn extend(
    language: &str,
    tree: &Tree,
    source: &str,
    chunks: &[Chunk],
    definitions: &mut Vec<IndexedDefinition>,
) -> Vec<IndexedDeclaration> {
    let kinds: &[&str] = match language {
        "c" | "cpp" => &["function_definition"],
        "go" => &["function_declaration", "method_declaration"],
        "java" => &[
            "method_declaration",
            "constructor_declaration",
            "compact_constructor_declaration",
        ],
        "rust" | "javascript" | "typescript" | "python" => &[],
        _ => return Vec::new(),
    };
    let mut declarations = Vec::new();
    let mut cursor = tree.walk();
    loop {
        let node = cursor.node();
        if let Some(declaration) = scoped_declaration(language, node, source) {
            declarations.push(declaration);
        }
        if let Some((name, qualified)) = enum_member(node, source) {
            if let Some(chunk) = chunk_of(chunks, node, name) {
                for symbol in [name.to_owned(), qualified] {
                    definitions.push(IndexedDefinition {
                        symbol,
                        chunk,
                        implementation: false,
                        member: true,
                    });
                }
            }
        }
        if kinds.contains(&node.kind()) {
            let name = node
                .child_by_field_name("name")
                .or_else(|| declarator_name(node.child_by_field_name("declarator")?));
            if let Some(name) = name {
                let position = name;
                let name = &source[name.byte_range()];
                if let Some(index) = chunk_of(chunks, position, name) {
                    if !definitions
                        .iter()
                        .any(|entry| entry.chunk == index && entry.symbol == name)
                    {
                        definitions.push(IndexedDefinition {
                            symbol: name.to_owned(),
                            chunk: index,
                            implementation: node.child_by_field_name("body").is_some(),
                            member: false,
                        });
                    }
                }
            }
        }
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return declarations;
            }
        }
    }
}

fn chunk_of(chunks: &[Chunk], node: Node<'_>, name: &str) -> Option<usize> {
    let line = node.start_position().row + 1;
    let index = chunks.partition_point(|chunk| chunk.end_line < line);
    // A syntax boundary can split one source line across chunks.
    let offset = chunks[index..]
        .iter()
        .take_while(|chunk| chunk.start_line <= line)
        .position(|chunk| chunk.content.contains(name))?;
    Some(index + offset)
}

/// The member's own name and `Enum.Member`.
fn enum_member<'source>(node: Node<'_>, source: &'source str) -> Option<(&'source str, String)> {
    let name = match node.kind() {
        "enum_variant" | "enum_constant" | "enumerator" | "enum_assignment" => {
            node.child_by_field_name("name")?
        }
        "property_identifier" if node.parent()?.kind() == "enum_body" => node,
        _ => return None,
    };
    let owner = std::iter::successors(node.parent(), Node::parent).find(|parent| {
        matches!(
            parent.kind(),
            "enum_item" | "enum_declaration" | "enum_specifier"
        )
    })?;
    let name = &source[name.byte_range()];
    // A C enum may be anonymous; its members are still known by name.
    let qualified = owner.child_by_field_name("name").map_or_else(
        || name.to_owned(),
        |owner| format!("{}.{name}", &source[owner.byte_range()]),
    );
    Some((name, qualified))
}

fn scoped_declaration(language: &str, node: Node<'_>, source: &str) -> Option<IndexedDeclaration> {
    let (method, owners): (_, &[&str]) = match language {
        "typescript" | "javascript" => (
            matches!(
                node.kind(),
                "method_definition" | "method_signature" | "abstract_method_signature"
            ) && matches!(node.parent()?.kind(), "class_body" | "interface_body")
                && matches!(
                    node.parent()?.parent()?.kind(),
                    "class_declaration" | "abstract_class_declaration" | "interface_declaration"
                ),
            &[
                "class_declaration",
                "abstract_class_declaration",
                "interface_declaration",
                "internal_module",
                "function_declaration",
            ],
        ),
        "python" => (
            node.kind() == "function_definition",
            &["class_definition", "function_definition"],
        ),
        _ => return None,
    };
    if !method {
        return None;
    }
    let name = node.child_by_field_name("name")?;
    let mut parts = vec![identifier(name, source)?];
    for parent in std::iter::successors(node.parent(), Node::parent) {
        if owners.contains(&parent.kind()) {
            parts.push(identifier(parent.child_by_field_name("name")?, source)?);
        }
    }
    if parts.len() < 2 {
        return None;
    }
    parts.reverse();
    let start = node.start_position().row + 1;
    let end = node.end_position();
    Some(IndexedDeclaration {
        qualified_name: parts.join("."),
        span: LineSpan {
            start,
            end: (end.row + usize::from(end.column != 0)).max(start),
        },
    })
}

fn identifier<'source>(node: Node<'_>, source: &'source str) -> Option<&'source str> {
    matches!(
        node.kind(),
        "identifier" | "type_identifier" | "property_identifier" | "private_property_identifier"
    )
    .then(|| &source[node.byte_range()])
}

fn declarator_name(mut node: Node<'_>) -> Option<Node<'_>> {
    loop {
        node = match node.kind() {
            "identifier" | "field_identifier" | "type_identifier" | "destructor_name"
            | "operator_name" => return Some(node),
            "qualified_identifier" | "template_function" | "template_method" => {
                node.child_by_field_name("name")?
            }
            "parenthesized_declarator" | "reference_declarator" => node.named_child(0)?,
            _ => node.child_by_field_name("declarator")?,
        };
    }
}
