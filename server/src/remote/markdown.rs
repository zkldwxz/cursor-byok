//! Reads Markdown as the flat blocks the phone draws as text.
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum Block {
    Text {
        runs: Vec<Run>,
    },
    Heading {
        runs: Vec<Run>,
    },
    Quote {
        runs: Vec<Run>,
    },
    Item {
        runs: Vec<Run>,
        depth: usize,
        marker: String,
    },
    Code {
        text: String,
    },
    Table {
        rows: Vec<Vec<String>>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(super) struct Run {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<&'static str>,
}

#[derive(Default)]
struct Reader {
    blocks: Vec<Block>,
    runs: Vec<Run>,
    styles: Vec<&'static str>,
    /// Innermost last; a number counts an ordered list's items.
    lists: Vec<Option<u64>>,
    marker: Option<String>,
    quotes: usize,
    heading: bool,
    code: Option<String>,
    table: Option<Vec<Vec<String>>>,
}

pub(super) fn blocks(markdown: &str) -> Vec<Block> {
    let mut reader = Reader::default();
    for event in Parser::new_ext(
        markdown,
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH,
    ) {
        reader.read(event);
    }
    reader.flush();
    reader.blocks
}

impl Reader {
    fn read(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) | Event::InlineMath(text) | Event::DisplayMath(text) => {
                self.text(&text, self.styles.last().copied())
            }
            Event::Code(text) => self.text(&text, Some("code")),
            // Shown as written: the phone draws no markup of the agent's.
            Event::Html(text) | Event::InlineHtml(text) => self.text(&text, None),
            Event::SoftBreak => self.text(" ", None),
            Event::HardBreak => self.text("\n", None),
            Event::Rule | Event::FootnoteReference(_) | Event::TaskListMarker(_) => {}
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Heading { level, .. } => {
                self.flush();
                self.heading = level <= HeadingLevel::H6;
            }
            Tag::BlockQuote(_) => {
                self.flush();
                self.quotes += 1;
            }
            Tag::CodeBlock(_) => {
                self.flush();
                self.code = Some(String::new());
            }
            Tag::List(first) => {
                self.flush();
                self.lists.push(first);
            }
            Tag::Item => {
                self.flush();
                self.marker = Some(match self.lists.last_mut() {
                    Some(Some(number)) => {
                        *number += 1;
                        format!("{}.", *number - 1)
                    }
                    _ => "•".into(),
                });
            }
            Tag::Table(_) => {
                self.flush();
                self.table = Some(Vec::new());
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some(rows) = &mut self.table {
                    rows.push(Vec::new());
                }
            }
            Tag::Strong => self.styles.push("strong"),
            Tag::Emphasis => self.styles.push("em"),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Item | TagEnd::HtmlBlock => self.flush(),
            TagEnd::Heading(_) => {
                self.flush();
                self.heading = false;
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.quotes -= 1;
            }
            TagEnd::CodeBlock => {
                let text = self.code.take().unwrap_or_default();
                let text = text.trim_end_matches('\n').to_owned();
                self.blocks.push(Block::Code { text });
            }
            TagEnd::List(_) => {
                self.lists.pop();
            }
            TagEnd::TableCell => {
                let cell = self.runs.drain(..).map(|run| run.text).collect();
                if let Some(row) = self.table.as_mut().and_then(|rows| rows.last_mut()) {
                    row.push(cell);
                }
            }
            TagEnd::Table => {
                let rows = self.table.take().unwrap_or_default();
                self.blocks.push(Block::Table { rows });
            }
            TagEnd::Strong | TagEnd::Emphasis => {
                self.styles.pop();
            }
            _ => {}
        }
    }

    fn text(&mut self, text: &str, style: Option<&'static str>) {
        if let Some(code) = &mut self.code {
            code.push_str(text);
            return;
        }
        match self.runs.last_mut() {
            Some(last) if last.style == style => last.text.push_str(text),
            _ => self.runs.push(Run {
                text: text.into(),
                style,
            }),
        }
    }

    fn flush(&mut self) {
        let runs = std::mem::take(&mut self.runs);
        if runs.iter().all(|run| run.text.trim().is_empty()) {
            return;
        }
        let block = if self.heading {
            Block::Heading { runs }
        } else if let Some(marker) = self.marker.take() {
            Block::Item {
                runs,
                depth: self.lists.len().saturating_sub(1),
                marker,
            }
        } else if self.quotes > 0 {
            Block::Quote { runs }
        } else {
            Block::Text { runs }
        };
        self.blocks.push(block);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn prose_lists_code_and_tables_keep_their_order_and_styles() {
        let read = blocks(
            "# Title\n\nSome **bold** and `code`.\n\n1. one\n   - inner\n2. two\n\n```rust\nlet a = 1;\n```\n\n> said\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
        );
        assert_eq!(
            serde_json::to_value(read).unwrap(),
            json!([
                {"kind": "heading", "runs": [{"text": "Title"}]},
                {"kind": "text", "runs": [
                    {"text": "Some "}, {"text": "bold", "style": "strong"}, {"text": " and "},
                    {"text": "code", "style": "code"}, {"text": "."},
                ]},
                {"kind": "item", "runs": [{"text": "one"}], "depth": 0, "marker": "1."},
                {"kind": "item", "runs": [{"text": "inner"}], "depth": 1, "marker": "•"},
                {"kind": "item", "runs": [{"text": "two"}], "depth": 0, "marker": "2."},
                {"kind": "code", "text": "let a = 1;"},
                {"kind": "quote", "runs": [{"text": "said"}]},
                {"kind": "table", "rows": [["a", "b"], ["1", "2"]]},
            ])
        );
    }

    #[test]
    fn an_unfinished_reply_is_still_readable() {
        assert_eq!(
            blocks("text\n\n```\npartial"),
            vec![
                Block::Text {
                    runs: vec![Run {
                        text: "text".into(),
                        style: None
                    }]
                },
                Block::Code {
                    text: "partial".into()
                },
            ]
        );
    }

    #[test]
    fn markup_in_a_reply_stays_text() {
        assert_eq!(
            blocks("a <b>x</b>"),
            vec![Block::Text {
                runs: vec![Run {
                    text: "a <b>x</b>".into(),
                    style: None
                }]
            }]
        );
    }
}
