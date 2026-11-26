//! Recursive Tree-sitter boundary selection and public chunk construction.

use tree_sitter::{Node, Tree};

use crate::{language::parser_for, types::Chunk};

use super::{boundary::merge_adjacent, line_boundaries, ChunkBoundary};

const MAX_DEPTH: usize = 500;
const MIN_CHUNK_BYTES: usize = 50;

pub fn chunk_source(
    source: &str,
    file_path: &str,
    language: Option<&str>,
    desired: usize,
) -> Vec<Chunk> {
    let tree = language
        .and_then(parser_for)
        .and_then(|mut parser| parser.parse(source, None));
    chunk_parsed_source(source, file_path, language, desired, tree.as_ref())
        .into_iter()
        .map(|(chunk, _)| chunk)
        .collect()
}

pub(crate) fn chunk_parsed_source(
    source: &str,
    file_path: &str,
    language: Option<&str>,
    desired: usize,
    tree: Option<&Tree>,
) -> Vec<(Chunk, ChunkBoundary)> {
    if source.trim().is_empty() {
        return Vec::new();
    }
    let boundaries = tree
        .map(|tree| merge_adjacent(split_node(tree.root_node(), desired, 0, None), desired))
        .filter(|items| !items.is_empty())
        .unwrap_or_else(|| line_boundaries(source, desired));
    let mut offset = 0;
    let mut line = 1;
    boundaries
        .into_iter()
        .filter_map(|boundary| {
            let content = source.get(boundary.start..boundary.end)?;
            line += newline_count(&source[offset..boundary.start]);
            let start_line = line;
            let newlines = newline_count(content);
            let end_line = start_line + newlines - usize::from(content.ends_with('\n'));
            line += newlines;
            offset = boundary.end;
            Some((
                Chunk {
                    file_path: file_path.to_owned(),
                    start_line,
                    end_line,
                    language: language.map(str::to_owned),
                    content: content.to_owned(),
                },
                boundary,
            ))
        })
        .collect()
}

fn newline_count(source: &str) -> usize {
    source.bytes().filter(|byte| *byte == b'\n').count()
}

fn split_node(
    node: Node<'_>,
    desired: usize,
    depth: usize,
    lead: Option<usize>,
) -> Vec<ChunkBoundary> {
    if node.child_count() == 0
        || depth > MAX_DEPTH
        || node.end_byte().saturating_sub(node.start_byte()) < MIN_CHUNK_BYTES
    {
        return vec![ChunkBoundary {
            start: lead.unwrap_or(node.start_byte()),
            end: node.end_byte(),
        }];
    }
    // A definition's decorators and signature are too small to stand alone; split
    // off, they end up at the tail of the previous definition's chunk.
    let body = node
        .child_by_field_name("body")
        .or_else(|| node.child_by_field_name("definition"));
    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    let mut output = Vec::new();
    let mut lead = lead;
    let mut index = 0;
    while index < children.len() {
        let child = children[index];
        index += 1;
        let start = lead.unwrap_or(child.start_byte());
        if child.end_byte().saturating_sub(start) > desired {
            output.extend(split_node(child, desired, depth + 1, lead.take()));
            continue;
        }
        lead = None;
        let mut end = child.end_byte();
        while index < children.len() && children[index].end_byte().saturating_sub(start) <= desired
        {
            end = children[index].end_byte();
            index += 1;
        }
        if body.is_some_and(|body| children.get(index) == Some(&body)) {
            lead = Some(start);
            continue;
        }
        output.push(ChunkBoundary { start, end });
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_chunks_keep_paths_lines_and_source_text() {
        let source =
            "fn first() {\n    println!(\"one\");\n}\n\nfn second() {\n    println!(\"two\");\n}\n";
        let chunks = chunk_source(source, "src/lib.rs", Some("rust"), 48);
        assert!(chunks.len() >= 2);
        assert_eq!(chunks[0].file_path, "src/lib.rs");
        assert_eq!(chunks[0].start_line, 1);
        assert!(chunks.iter().any(|chunk| chunk.content.contains("second")));
        assert!(chunks.iter().all(|chunk| !chunk.content.is_empty()));
    }

    #[test]
    fn unsupported_languages_fall_back_to_line_chunks() {
        let source = "alpha beta gamma\ndelta epsilon\nzeta eta theta\n";
        let chunks = chunk_source(source, "notes.unknown", None, 24);
        assert!(chunks.len() >= 2);
        assert_eq!(chunks[0].start_line, 1);
        assert_eq!(chunks.last().unwrap().end_line, 3);
    }

    #[test]
    fn a_split_function_keeps_its_signature_with_the_start_of_its_body() {
        let source = concat!(
            "def small(): return 1\n",
            "def large(\n",
            "    first,\n",
            "    second,\n",
            "):\n",
            "    alpha = first\n",
            "    beta = second\n",
            "    gamma = alpha + beta\n",
            "    delta = gamma * 2\n",
            "    epsilon = delta - 1\n",
            "    zeta = epsilon + 2\n",
            "    eta = zeta * 3\n",
            "    theta = eta - 4\n",
            "    iota = theta + 5\n",
            "    return iota\n",
        );
        let chunks = chunk_source(source, "lib.py", Some("python"), 120);
        let large = chunks
            .iter()
            .find(|chunk| chunk.content.contains("def large("))
            .expect("the split function must appear in some chunk");
        assert!(
            large.content.contains("alpha = first"),
            "the chunk carrying the signature must open the body, got:\n{}",
            large.content
        );
    }

    #[test]
    fn a_decorated_split_function_keeps_decorator_signature_and_body_start_together() {
        let source = concat!(
            "def small(): return 1\n",
            "@cached\n",
            "def large(\n",
            "    first,\n",
            "    second,\n",
            "):\n",
            "    alpha = first\n",
            "    beta = second\n",
            "    gamma = alpha + beta\n",
            "    delta = gamma * 2\n",
            "    epsilon = delta - 1\n",
            "    zeta = epsilon + 2\n",
            "    eta = zeta * 3\n",
            "    theta = eta - 4\n",
            "    iota = theta + 5\n",
            "    return iota\n",
        );
        let chunks = chunk_source(source, "lib.py", Some("python"), 120);
        let decorated = chunks
            .iter()
            .find(|chunk| chunk.content.contains("@cached"))
            .expect("the decorator must appear in some chunk");
        assert!(
            decorated.content.contains("def large(") && decorated.content.contains("alpha = first"),
            "the decorator, the signature and the body start must share a chunk, got:\n{}",
            decorated.content
        );
    }

    #[test]
    fn multibyte_character_at_a_chunk_boundary_does_not_panic() {
        let source = "// 中文说明。";
        let chunks = chunk_source(source, "src/lib.rs", Some("rust"), 750);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].content, source);
        assert_eq!(chunks[0].start_line, 1);
        assert_eq!(chunks[0].end_line, 1);
    }
}
