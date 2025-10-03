use pulldown_cmark::{CodeBlockKind, CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::layout::Alignment;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};

#[derive(Default)]
pub struct Renderer;

impl Renderer {
    pub fn new() -> Self {
        Self
    }

    pub fn render(&self, input: &str) -> Text<'static> {
        let mut opts = Options::empty();
        opts.insert(Options::ENABLE_STRIKETHROUGH);
        opts.insert(Options::ENABLE_TABLES);
        opts.insert(Options::ENABLE_TASKLISTS);
        opts.insert(Options::ENABLE_FOOTNOTES);

        let parser = Parser::new_ext(input, opts);
        let mut writer = MarkdownWriter::default();
        writer.render(parser);
        writer.into_text()
    }
}

#[derive(Default)]
struct MarkdownWriter {
    lines: Vec<RenderLine>,
    buffer: String,
    list_stack: Vec<ListState>,
    current_item: Option<ListMeta>,
    blockquote_depth: usize,
    heading_level: Option<u8>,
    in_paragraph: bool,
    code_block: Option<CodeMeta>,
    link_target: Option<String>,
}

#[derive(Clone, Copy)]
struct ListState {
    ordered: bool,
    index: usize,
}

#[derive(Clone)]
struct ListMeta {
    indent: usize,
    marker: String,
}

#[derive(Default)]
struct CodeMeta {
    language: Option<String>,
    buffer: String,
}

#[derive(Clone)]
enum RenderLine {
    Text(String),
    Heading {
        level: u8,
        text: String,
    },
    Bullet {
        indent: usize,
        marker: String,
        text: String,
    },
    Quote {
        depth: usize,
        text: String,
    },
    Code(String),
    Separator,
}

impl MarkdownWriter {
    fn render<'a, I>(&mut self, parser: I)
    where
        I: Iterator<Item = Event<'a>>,
    {
        for event in parser {
            match event {
                Event::Start(tag) => self.start_tag(tag),
                Event::End(tag) => self.end_tag(tag),
                Event::Text(text) => self.text(text),
                Event::Code(code) => self.inline_code(code),
                Event::Html(_) | Event::InlineHtml(_) => {}
                Event::FootnoteReference(name) => self.append_text(format!("[{}]", name)),
                Event::HardBreak => self.append_text("\n"),
                Event::SoftBreak => self.append_text(" "),
                Event::Rule => {
                    self.flush_buffer();
                    self.lines.push(RenderLine::Text("―".repeat(20)));
                    self.lines.push(RenderLine::Separator);
                }
                Event::TaskListMarker(done) => {
                    self.append_text(if done { "[x] " } else { "[ ] " });
                }
            }
        }
        self.flush_buffer();
    }

    fn start_tag<'a>(&mut self, tag: Tag<'a>) {
        match tag {
            Tag::Paragraph => {
                self.flush_buffer();
                self.in_paragraph = true;
            }
            Tag::Heading { level, .. } => {
                self.flush_buffer();
                self.heading_level = Some(level_to_u8(level));
            }
            Tag::BlockQuote => {
                self.flush_buffer();
                self.blockquote_depth += 1;
            }
            Tag::CodeBlock(kind) => {
                self.flush_buffer();
                let language = match kind {
                    CodeBlockKind::Fenced(lang) if !lang.is_empty() => Some(lang.into_string()),
                    _ => None,
                };
                self.code_block = Some(CodeMeta {
                    language,
                    buffer: String::new(),
                });
            }
            Tag::List(start) => {
                let index = start.unwrap_or(1) as usize;
                self.list_stack.push(ListState {
                    ordered: start.is_some(),
                    index,
                });
            }
            Tag::Item => {
                self.flush_buffer();
                let indent = self.list_stack.len().saturating_sub(1);
                if let Some(state) = self.list_stack.last() {
                    let marker = if state.ordered {
                        format!("{}.", state.index)
                    } else {
                        "•".to_string()
                    };
                    self.current_item = Some(ListMeta { indent, marker });
                }
            }
            Tag::Emphasis | Tag::Strong | Tag::Strikethrough => {}
            Tag::Link { dest_url, .. } => {
                self.link_target = Some(dest_url.into_string());
            }
            Tag::Image { .. } => {
                self.append_text("[image available]");
            }
            Tag::Table(_) | Tag::TableHead | Tag::TableRow | Tag::TableCell => {
                self.append_text("| ");
            }
            Tag::FootnoteDefinition(_) => {}
            Tag::HtmlBlock => {}
            Tag::MetadataBlock(_) => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush_buffer();
                self.in_paragraph = false;
                self.lines.push(RenderLine::Separator);
            }
            TagEnd::Heading(_) => {
                if let Some(level) = self.heading_level.take() {
                    let text = self.consume_buffer();
                    if !text.is_empty() {
                        self.lines.push(RenderLine::Heading { level, text });
                        self.lines.push(RenderLine::Separator);
                    }
                }
            }
            TagEnd::BlockQuote => {
                self.flush_buffer();
                if self.blockquote_depth > 0 {
                    self.blockquote_depth -= 1;
                }
                self.lines.push(RenderLine::Separator);
            }
            TagEnd::CodeBlock => {
                if let Some(mut meta) = self.code_block.take() {
                    if let Some(lang) = meta.language.take() {
                        self.lines.push(RenderLine::Text(format!("```{}", lang)));
                    } else {
                        self.lines.push(RenderLine::Text("```".to_string()));
                    }
                    for line in meta.buffer.split('\n') {
                        self.lines.push(RenderLine::Code(line.to_string()));
                    }
                    self.lines.push(RenderLine::Text("```".to_string()));
                    self.lines.push(RenderLine::Separator);
                }
            }
            TagEnd::List(_) => {
                self.flush_buffer();
                self.list_stack.pop();
                self.lines.push(RenderLine::Separator);
            }
            TagEnd::Item => {
                self.flush_buffer();
                if let Some(state) = self.list_stack.last_mut() {
                    state.index += 1;
                }
                self.current_item = None;
            }
            TagEnd::Link => {
                self.link_target = None;
            }
            TagEnd::Table | TagEnd::TableHead | TagEnd::TableRow | TagEnd::TableCell => {}
            TagEnd::FootnoteDefinition => {}
            _ => {}
        }
    }

    fn text<'a>(&mut self, text: CowStr<'a>) {
        if let Some(code) = self.code_block.as_mut() {
            code.buffer.push_str(&text);
        } else {
            self.append_text(text);
        }
    }

    fn inline_code<'a>(&mut self, code: CowStr<'a>) {
        self.append_text(format!("`{}`", code));
    }

    fn append_text<T: AsRef<str>>(&mut self, text: T) {
        self.buffer.push_str(text.as_ref());
    }

    fn flush_buffer(&mut self) {
        let text = self.consume_buffer();
        if text.is_empty() {
            return;
        }

        if let Some(level) = self.heading_level {
            self.lines.push(RenderLine::Heading { level, text });
            return;
        }

        if let Some(code) = self.code_block.as_mut() {
            if !code.buffer.is_empty() {
                code.buffer.push('\n');
            }
            code.buffer.push_str(&text);
            return;
        }

        if let Some(item) = &self.current_item {
            self.lines.push(RenderLine::Bullet {
                indent: item.indent,
                marker: item.marker.clone(),
                text,
            });
            return;
        }

        if self.blockquote_depth > 0 {
            self.lines.push(RenderLine::Quote {
                depth: self.blockquote_depth,
                text,
            });
            return;
        }

        self.lines.push(RenderLine::Text(text));
    }

    fn consume_buffer(&mut self) -> String {
        let text = self.buffer.trim().to_string();
        self.buffer.clear();
        text
    }

    fn into_text(mut self) -> Text<'static> {
        while matches!(self.lines.last(), Some(RenderLine::Separator)) {
            self.lines.pop();
        }

        let mut styled_lines = Vec::with_capacity(self.lines.len());
        for line in self.lines {
            match line {
                RenderLine::Text(content) => styled_lines.push(Line::from(Span::raw(content))),
                RenderLine::Heading { level, text } => {
                    let style = heading_style(level);
                    styled_lines.push(Line::from(Span::styled(text, style)));
                }
                RenderLine::Bullet {
                    indent,
                    marker,
                    text,
                } => {
                    let mut spans = Vec::new();
                    spans.push(Span::raw("  ".repeat(indent)));
                    spans.push(Span::styled(
                        format!("{} ", marker),
                        Style::default().fg(Color::Yellow),
                    ));
                    spans.push(Span::raw(text));
                    styled_lines.push(Line::from(spans));
                }
                RenderLine::Quote { depth, text } => {
                    let prefix = ">".repeat(depth.max(1));
                    styled_lines.push(Line::from(vec![
                        Span::styled(prefix + " ", Style::default().fg(Color::Green)),
                        Span::styled(text, Style::default().fg(Color::Green)),
                    ]));
                }
                RenderLine::Code(text) => {
                    styled_lines.push(Line::from(vec![Span::styled(
                        text,
                        Style::default().fg(Color::Cyan),
                    )]));
                }
                RenderLine::Separator => styled_lines.push(Line::default()),
            }
        }

        if styled_lines.is_empty() {
            styled_lines.push(Line::from(Span::raw("")));
        }

        Text {
            lines: styled_lines,
            alignment: Some(Alignment::Left),
            style: Style::default(),
        }
    }
}

fn heading_style(level: u8) -> Style {
    match level {
        1 => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        2 => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
        3 => Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
        _ => Style::default().fg(Color::Magenta),
    }
}

fn level_to_u8(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}
