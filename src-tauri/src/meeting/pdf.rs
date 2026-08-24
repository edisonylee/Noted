use std::path::Path;

use anyhow::Result;
use chrono::DateTime;
use printpdf::{
    path::{PaintMode, WindingOrder},
    BuiltinFont, Color, Line, LineCapStyle, Mm, PdfDocument, PdfDocumentReference,
    PdfLayerReference, Point, Polygon, Rgb,
};
use serde_json::Value;

const PAGE_H: f32 = 297.0;
const LEFT: f32 = 24.0;
const RIGHT: f32 = 24.0;
const TOP: f32 = 270.0;
const BOTTOM: f32 = 18.0;
const BODY_W: f32 = 210.0 - LEFT - RIGHT;
const COLUMN_GAP: f32 = 10.0;
const COLUMN_W: f32 = (BODY_W - COLUMN_GAP) / 2.0;

const INK: (f32, f32, f32) = (0.12, 0.11, 0.10);
const MUTED: (f32, f32, f32) = (0.43, 0.41, 0.38);
const QUIET: (f32, f32, f32) = (0.58, 0.55, 0.51);
const RULE: (f32, f32, f32) = (0.82, 0.80, 0.77);
const PAPER: (f32, f32, f32) = (0.969, 0.961, 0.945);
const ACCENT: (f32, f32, f32) = (0.239, 0.475, 0.741);
const ACCENT_DARK: (f32, f32, f32) = (0.18, 0.35, 0.56);
const ACCENT_TINT: (f32, f32, f32) = (0.882, 0.918, 0.957);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExportKind {
    Notes,
    Transcript,
}

#[derive(Clone, Copy, Debug)]
pub struct ExportOptions {
    pub kind: ExportKind,
    pub summary_id: Option<i64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExportStats {
    pub pages: usize,
    pub transcript_turns: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TranscriptTurn {
    speaker: String,
    start_ms: i64,
    text: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MarkdownSection {
    heading: String,
    body: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct NotesCounts {
    decisions: usize,
    next_moves: usize,
    open_threads: usize,
}

fn clean(s: &str) -> String {
    s.replace("**", "")
        .replace('—', "-")
        .replace('–', "-")
        .replace('“', "\"")
        .replace('”', "\"")
        .replace('’', "'")
        .replace('•', "-")
        .replace('□', "[ ]")
        .replace('■', "[x]")
}

fn wrap(text: &str, max: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in clean(text).split_whitespace() {
        if !line.is_empty() && line.chars().count() + word.chars().count() + 1 > max {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

fn markdown_sections(markdown: &str) -> Vec<MarkdownSection> {
    let mut sections = Vec::new();
    let mut heading = String::new();
    let mut body = Vec::new();

    let flush =
        |sections: &mut Vec<MarkdownSection>, heading: &mut String, body: &mut Vec<&str>| {
            let content = body.join("\n").trim().to_string();
            if !heading.is_empty() || !content.is_empty() {
                sections.push(MarkdownSection {
                    heading: std::mem::take(heading),
                    body: content,
                });
            }
            body.clear();
        };

    for raw in markdown.lines() {
        if let Some(next_heading) = raw
            .trim()
            .strip_prefix("## ")
            .or_else(|| raw.trim().strip_prefix("### "))
        {
            flush(&mut sections, &mut heading, &mut body);
            heading = clean(next_heading.trim());
        } else {
            body.push(raw);
        }
    }
    flush(&mut sections, &mut heading, &mut body);
    sections
}

fn first_prose(section: &MarkdownSection) -> String {
    let mut lines = Vec::new();
    for raw in section.body.lines() {
        let line = raw.trim();
        if line.is_empty() {
            if !lines.is_empty() {
                break;
            }
            continue;
        }
        let prose = line
            .strip_prefix("- [ ] ")
            .or_else(|| line.strip_prefix("- [x] "))
            .or_else(|| line.strip_prefix("- "))
            .unwrap_or(line);
        lines.push(clean(prose));
    }
    lines
        .join(" ")
        .split_whitespace()
        .filter(|word| {
            let source = word.trim_matches(['[', ']']);
            !source.eq_ignore_ascii_case("notes")
                && !source.split_once(':').is_some_and(|(minutes, seconds)| {
                    !minutes.is_empty()
                        && !seconds.is_empty()
                        && minutes.chars().all(|ch| ch.is_ascii_digit())
                        && seconds.chars().all(|ch| ch.is_ascii_digit())
                })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn section_item_count(section: &MarkdownSection) -> usize {
    let count = section
        .body
        .lines()
        .filter(|line| {
            let line = line.trim();
            line.starts_with("- ")
                || line.split_once(". ").is_some_and(|(number, text)| {
                    !text.is_empty()
                        && !number.is_empty()
                        && number.chars().all(|ch| ch.is_ascii_digit())
                })
        })
        .count();
    if count == 0 && !section.body.trim().is_empty() {
        1
    } else {
        count
    }
}

fn notes_counts(sections: &[MarkdownSection]) -> NotesCounts {
    let mut counts = NotesCounts::default();
    for section in sections {
        let heading = section.heading.to_lowercase();
        let items = section_item_count(section);
        if heading.contains("decision") {
            counts.decisions += items;
        }
        if [
            "action",
            "commitment",
            "follow-up",
            "follow up",
            "next",
            "experiment",
            "milestone",
        ]
        .iter()
        .any(|needle| heading.contains(needle))
        {
            counts.next_moves += items;
        }
        if [
            "risk",
            "question",
            "blocker",
            "concern",
            "gap",
            "revisit",
            "parking lot",
        ]
        .iter()
        .any(|needle| heading.contains(needle))
        {
            counts.open_threads += items;
        }
    }
    counts
}

fn count_label(count: usize, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}

fn save_standard_pdf(doc: PdfDocumentReference, path: &Path) -> Result<()> {
    let bytes = doc.save_to_bytes()?;
    let mut pdf = printpdf::lopdf::Document::load_mem(&bytes)?;
    let info_id = pdf
        .trailer
        .get(b"Info")
        .ok()
        .and_then(|info| info.as_reference().ok());
    if let Some(info_id) = info_id {
        if let Ok(info) = pdf
            .get_object_mut(info_id)
            .and_then(printpdf::lopdf::Object::as_dict_mut)
        {
            // printpdf writes empty conformance and document-info keys. Some
            // viewers misidentify them as custom or PDF/X-9 metadata.
            for key in [
                b"GTS_PDFXVersion".as_slice(),
                b"Trapped".as_slice(),
                b"Author".as_slice(),
                b"Creator".as_slice(),
                b"Producer".as_slice(),
                b"Subject".as_slice(),
                b"Identifier".as_slice(),
                b"Keywords".as_slice(),
            ] {
                info.remove(key);
            }
        }
    }
    pdf.save(path)?;
    Ok(())
}

fn display_date(raw: &str) -> String {
    DateTime::parse_from_rfc3339(raw)
        .map(|date| date.format("%B %-d, %Y at %-I:%M %p").to_string())
        .unwrap_or_else(|_| raw.replace('T', " ").chars().take(16).collect())
}

fn meeting_duration(meeting: &Value) -> Option<String> {
    let ms = meeting["segments"].as_array().and_then(|segments| {
        segments
            .iter()
            .filter_map(|segment| segment["t1_ms"].as_i64())
            .max()
    })?;
    let minutes = (ms / 60_000).max(1);
    Some(if minutes >= 60 {
        format!("{}h {}m", minutes / 60, minutes % 60)
    } else {
        format!("{minutes} min")
    })
}

fn attendee_names(meeting: &Value) -> Vec<String> {
    meeting["event_json"]["attendees"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|attendee| !attendee["self"].as_bool().unwrap_or(false))
        .filter_map(|attendee| {
            attendee["name"]
                .as_str()
                .filter(|name| !name.trim().is_empty())
                .or_else(|| attendee["email"].as_str())
                .map(str::trim)
                .map(str::to_string)
        })
        .take(6)
        .collect()
}

fn selected_summary<'a>(meeting: &'a Value, summary_id: Option<i64>) -> Option<&'a Value> {
    let summaries = meeting["summaries"].as_array()?;
    summary_id
        .and_then(|id| {
            summaries
                .iter()
                .find(|summary| summary["id"].as_i64() == Some(id))
        })
        .or_else(|| summaries.first())
}

fn transcript_turns(meeting: &Value) -> Vec<TranscriptTurn> {
    let mut turns: Vec<TranscriptTurn> = Vec::new();
    for segment in meeting["segments"].as_array().into_iter().flatten() {
        let text = segment["text"].as_str().unwrap_or("").trim();
        if text.is_empty() {
            continue;
        }
        let speaker = if meeting["capture_mode"].as_str() == Some("in_person") {
            segment["speaker"].as_str().unwrap_or("Unassigned")
        } else if segment["channel"] == "me" {
            "Me"
        } else {
            segment["speaker"].as_str().unwrap_or("Them")
        };
        if let Some(previous) = turns.last_mut().filter(|turn| turn.speaker == speaker) {
            previous.text.push(' ');
            previous.text.push_str(text);
            continue;
        }
        turns.push(TranscriptTurn {
            speaker: speaker.to_string(),
            start_ms: segment["t0_ms"].as_i64().unwrap_or(0),
            text: text.to_string(),
        });
    }
    turns
}

struct Writer {
    doc: PdfDocumentReference,
    layer: PdfLayerReference,
    regular: printpdf::IndirectFontRef,
    bold: printpdf::IndirectFontRef,
    y: f32,
    page: usize,
    document_label: &'static str,
    column: usize,
    column_top: f32,
}

impl Writer {
    fn new(title: &str, document_label: &'static str) -> Result<Self> {
        let (doc, page, layer) = PdfDocument::new(title, Mm(210.0), Mm(PAGE_H), document_label);
        let regular = doc.add_builtin_font(BuiltinFont::Helvetica)?;
        let bold = doc.add_builtin_font(BuiltinFont::HelveticaBold)?;
        let mut out = Self {
            layer: doc.get_page(page).get_layer(layer),
            doc,
            regular,
            bold,
            y: TOP,
            page: 1,
            document_label,
            column: 0,
            column_top: TOP,
        };
        out.page_frame();
        Ok(out)
    }

    fn page_frame(&mut self) {
        self.fill_polygon(
            &[(0.0, 0.0), (210.0, 0.0), (210.0, PAGE_H), (0.0, PAGE_H)],
            PAPER,
        );

        self.draw_line("noted", 8.3, true, INK, LEFT, 282.0);
        self.fill_polygon(
            &[(36.3, 282.1), (38.0, 282.1), (38.0, 283.8), (36.3, 283.8)],
            ACCENT,
        );
        self.draw_line(
            if self.document_label == "Transcript" {
                "Full transcript"
            } else {
                "Meeting notes"
            },
            7.4,
            false,
            MUTED,
            166.0,
            282.0,
        );

        self.draw_line(&format!("{:02}", self.page), 7.2, true, QUIET, 181.8, 12.0);
        self.fill_polygon(
            &[(187.3, 12.3), (189.0, 12.3), (189.0, 14.0), (187.3, 14.0)],
            ACCENT,
        );
        self.set_fill(INK);
    }

    fn next_page(&mut self) {
        let (page, layer) = self
            .doc
            .add_page(Mm(210.0), Mm(PAGE_H), self.document_label);
        self.layer = self.doc.get_page(page).get_layer(layer);
        self.page += 1;
        self.y = TOP;
        self.column = 0;
        self.column_top = TOP;
        self.page_frame();
    }

    fn ensure(&mut self, need: f32) {
        if self.y - need < BOTTOM {
            self.next_page();
        }
    }

    fn set_fill(&self, color: (f32, f32, f32)) {
        self.layer
            .set_fill_color(Color::Rgb(Rgb::new(color.0, color.1, color.2, None)));
    }

    fn set_outline(&self, color: (f32, f32, f32), thickness: f32) {
        self.layer
            .set_outline_color(Color::Rgb(Rgb::new(color.0, color.1, color.2, None)));
        self.layer.set_outline_thickness(thickness);
        self.layer.set_line_cap_style(LineCapStyle::Round);
    }

    fn fill_polygon(&self, points: &[(f32, f32)], color: (f32, f32, f32)) {
        self.set_fill(color);
        self.layer.add_polygon(Polygon {
            rings: vec![points
                .iter()
                .map(|(x, y)| (Point::new(Mm(*x), Mm(*y)), false))
                .collect()],
            mode: PaintMode::Fill,
            winding_order: WindingOrder::NonZero,
        });
    }

    fn stroke(&self, points: &[(f32, f32)]) {
        self.layer.add_line(Line {
            points: points
                .iter()
                .map(|(x, y)| (Point::new(Mm(*x), Mm(*y)), false))
                .collect(),
            is_closed: false,
        });
    }

    fn draw_line(&self, text: &str, size: f32, bold: bool, color: (f32, f32, f32), x: f32, y: f32) {
        self.set_fill(color);
        let font = if bold { &self.bold } else { &self.regular };
        self.layer.use_text(clean(text), size, Mm(x), Mm(y), font);
    }

    fn line(&mut self, text: &str, size: f32, bold: bool, color: (f32, f32, f32), x: f32) {
        self.draw_line(text, size, bold, color, x, self.y);
    }

    fn text_at(
        &mut self,
        text: &str,
        size: f32,
        bold: bool,
        color: (f32, f32, f32),
        x: f32,
        width: f32,
        leading: f32,
    ) {
        let chars = (width / (size * 0.19)).max(18.0) as usize;
        for line in wrap(text, chars) {
            self.ensure(leading + 0.8);
            self.line(&line, size, bold, color, x);
            self.y -= leading;
        }
    }

    fn text(&mut self, text: &str, size: f32, bold: bool, color: (f32, f32, f32), indent: f32) {
        self.text_at(
            text,
            size,
            bold,
            color,
            LEFT + indent,
            BODY_W - indent,
            size * 0.46,
        );
    }

    fn readout(&mut self, insight: &str, counts: NotesCounts) {
        let lines = wrap(insight, 67);
        let line_height = 6.0;
        let height = 24.0 + lines.len() as f32 * line_height + 12.0;
        self.ensure(height + 6.0);

        let top = self.y;
        let bottom = top - height;
        self.fill_polygon(
            &[
                (19.0, bottom),
                (19.0, top),
                (179.0, top),
                (191.0, top - 12.0),
                (191.0, bottom),
            ],
            ACCENT_TINT,
        );
        self.fill_polygon(
            &[
                (19.0, top - 6.0),
                (24.0, top - 6.0),
                (24.0, top - 1.0),
                (19.0, top - 1.0),
            ],
            ACCENT,
        );

        self.draw_line("The readout", 8.3, true, ACCENT_DARK, 28.0, top - 11.0);
        let mut line_y = top - 21.0;
        for line in lines {
            self.draw_line(&line, 14.2, true, INK, 28.0, line_y);
            line_y -= line_height;
        }
        let metrics = [
            count_label(counts.decisions, "decision", "decisions"),
            count_label(counts.next_moves, "next move", "next moves"),
            count_label(counts.open_threads, "open thread", "open threads"),
        ]
        .join("   /   ");
        self.draw_line(&metrics, 8.2, false, MUTED, 28.0, bottom + 8.0);
        self.y = bottom - 5.0;
    }

    fn editorial_label(&self, index: usize, heading: &str, top: f32, authored: bool) {
        self.draw_line(
            &format!("{:02}", index),
            16.0,
            true,
            if authored { ACCENT } else { RULE },
            LEFT,
            top,
        );
        let label_lines = wrap(heading, 22);
        let mut y = top - 7.0;
        for label in label_lines.into_iter().take(3) {
            self.draw_line(&label, 8.5, true, INK, LEFT, y);
            y -= 3.8;
        }
        if authored {
            self.draw_line("Written by you", 7.4, false, ACCENT_DARK, LEFT, y - 0.6);
        }
    }

    fn editorial_list_item(
        &mut self,
        text: &str,
        marker: &str,
        checked: Option<bool>,
        x: f32,
        width: f32,
        authored: bool,
    ) {
        let indent = 7.0;
        let size = 10.2;
        let leading = 4.8;
        let chars = ((width - indent) / (size * 0.19)).max(24.0) as usize;
        let lines = wrap(text, chars);
        self.ensure(leading * lines.len() as f32 + 1.2);
        let strong_lead = text.trim_start().starts_with("**");
        for (line_index, line) in lines.iter().enumerate() {
            if line_index == 0 {
                if let Some(is_checked) = checked {
                    let box_x = x + 0.5;
                    let box_y = self.y + 0.1;
                    let side = 3.2;
                    self.set_outline(if is_checked { ACCENT } else { RULE }, 0.7);
                    self.stroke(&[
                        (box_x, box_y),
                        (box_x + side, box_y),
                        (box_x + side, box_y + side),
                        (box_x, box_y + side),
                        (box_x, box_y),
                    ]);
                    if is_checked {
                        self.set_outline(ACCENT, 0.8);
                        self.stroke(&[
                            (box_x + 0.7, box_y + 1.5),
                            (box_x + 1.4, box_y + 0.8),
                            (box_x + 2.7, box_y + 2.4),
                        ]);
                    }
                } else if marker == "-" {
                    self.fill_polygon(
                        &[
                            (x + 1.0, self.y + 1.0),
                            (x + 2.7, self.y + 1.0),
                            (x + 2.7, self.y + 2.7),
                            (x + 1.0, self.y + 2.7),
                        ],
                        if authored { ACCENT } else { MUTED },
                    );
                } else {
                    self.line(
                        marker,
                        8.4,
                        true,
                        if authored { ACCENT_DARK } else { MUTED },
                        x + 0.2,
                    );
                }
            }
            self.line(line, size, strong_lead && line_index == 0, INK, x + indent);
            self.y -= leading;
        }
        self.y -= 1.3;
    }

    fn editorial_markdown(&mut self, markdown: &str, x: f32, width: f32, authored: bool) {
        for raw in markdown.lines() {
            let line = raw.trim();
            if line.is_empty() {
                self.y -= 2.0;
                continue;
            }
            if let Some(heading) = line
                .strip_prefix("## ")
                .or_else(|| line.strip_prefix("### "))
            {
                self.ensure(12.0);
                self.y -= 2.8;
                self.text_at(heading, 10.8, true, INK, x, width, 4.8);
                self.y -= 1.0;
            } else if let Some(item) = line.strip_prefix("- [ ] ") {
                self.editorial_list_item(item, "", Some(false), x, width, authored);
            } else if let Some(item) = line.strip_prefix("- [x] ") {
                self.editorial_list_item(item, "", Some(true), x, width, authored);
            } else if let Some(item) = line.strip_prefix("- ") {
                self.editorial_list_item(item, "-", None, x, width, authored);
            } else if let Some((number, item)) = line.split_once(". ").filter(|(number, item)| {
                !item.is_empty()
                    && !number.is_empty()
                    && number.chars().all(|ch| ch.is_ascii_digit())
            }) {
                self.editorial_list_item(item, &format!("{number}."), None, x, width, authored);
            } else if line.starts_with('|') && line.ends_with('|') {
                let cells = line
                    .trim_matches('|')
                    .split('|')
                    .map(str::trim)
                    .collect::<Vec<_>>();
                let divider = cells.iter().all(|cell| {
                    let cell = cell.trim_matches(':').trim();
                    cell.len() >= 3 && cell.chars().all(|ch| ch == '-')
                });
                if !divider {
                    self.text_at(&cells.join("  /  "), 9.2, false, INK, x, width, 4.4);
                    self.y -= 1.0;
                }
            } else {
                let strong_lead = line.starts_with("**");
                self.text_at(line, 10.3, strong_lead, INK, x, width, 4.8);
                self.y -= 1.1;
            }
        }
    }

    fn editorial_section(&mut self, index: usize, heading: &str, markdown: &str, authored: bool) {
        self.ensure(23.0);
        self.y -= 7.0;
        let top = self.y;
        self.editorial_label(index, heading, top, authored);
        self.editorial_markdown(markdown, 64.0, 127.0, authored);
        self.y = self.y.min(top - 16.0);
        self.y -= 2.5;
    }

    fn eyebrow(&mut self, name: &str) {
        self.ensure(10.0);
        self.y -= 2.0;
        self.text(name, 8.2, true, MUTED, 0.0);
        self.y -= 1.5;
    }

    fn begin_columns(&mut self) {
        self.column = 0;
        self.column_top = self.y;
    }

    fn next_column(&mut self) {
        if self.column == 0 {
            self.column = 1;
            self.y = self.column_top;
        } else {
            self.next_page();
            self.column_top = TOP;
        }
    }

    fn ensure_column(&mut self, need: f32) {
        if self.y - need < BOTTOM {
            self.next_column();
        }
    }

    fn column_text(&mut self, text: &str, size: f32, bold: bool, color: (f32, f32, f32)) {
        let chars = (COLUMN_W / (size * 0.19)).max(25.0) as usize;
        let leading = size * 0.38;
        for line in wrap(text, chars) {
            self.ensure_column(leading + 0.6);
            let x = LEFT + self.column as f32 * (COLUMN_W + COLUMN_GAP);
            self.line(&line, size, bold, color, x);
            self.y -= leading;
        }
    }

    fn transcript_turn(&mut self, turn: &TranscriptTurn) {
        let preview_lines = wrap(&turn.text, 54).len().min(2) as f32;
        self.ensure_column(3.2 + preview_lines * 3.05);
        let seconds = turn.start_ms.max(0) / 1000;
        self.column_text(
            &format!(
                "{}  /  {:02}:{:02}",
                turn.speaker,
                seconds / 60,
                seconds % 60
            ),
            7.2,
            true,
            MUTED,
        );
        self.y -= 0.2;
        self.column_text(&turn.text, 8.0, false, INK);
        self.y -= 1.0;
    }
}

fn render_notes(meeting: &Value, writer: &mut Writer, summary_id: Option<i64>) {
    if let Some(summary) = selected_summary(meeting, summary_id) {
        let sections = markdown_sections(summary["content_md"].as_str().unwrap_or(""));
        if let Some((featured_index, featured)) = sections
            .iter()
            .enumerate()
            .find(|(_, section)| !first_prose(section).is_empty())
        {
            writer.readout(&first_prose(featured), notes_counts(&sections));
            let mut index = 1;
            for (section_index, section) in sections.iter().enumerate() {
                if section_index == featured_index || section.body.trim().is_empty() {
                    continue;
                }
                let heading = if section.heading.is_empty() {
                    "Meeting detail"
                } else {
                    &section.heading
                };
                writer.editorial_section(index, heading, &section.body, false);
                index += 1;
            }
            if let Some(notes) = meeting["raw_notes"]
                .as_str()
                .filter(|notes| !notes.trim().is_empty())
            {
                writer.editorial_section(index, "Your notes", notes, true);
            }
            return;
        }
        writer.text(
            "This meeting does not have a readable summary yet.",
            10.2,
            false,
            MUTED,
            0.0,
        );
    } else {
        writer.text(
            "No summary was available for this meeting.",
            10.2,
            false,
            MUTED,
            0.0,
        );
    }
    if let Some(notes) = meeting["raw_notes"]
        .as_str()
        .filter(|notes| !notes.trim().is_empty())
    {
        writer.editorial_section(1, "Your notes", notes, true);
    }
}

fn render_transcript(meeting: &Value, writer: &mut Writer, turns: &[TranscriptTurn]) {
    writer.eyebrow("Full transcript");
    writer.text(
        &format!(
            "{} conversation turn{}{}",
            turns.len(),
            if turns.len() == 1 { "" } else { "s" },
            meeting_duration(meeting)
                .map(|duration| format!("  /  {duration}"))
                .unwrap_or_default()
        ),
        8.6,
        false,
        MUTED,
        0.0,
    );
    writer.y -= 4.0;
    writer.begin_columns();
    for turn in turns {
        writer.transcript_turn(turn);
    }
}

pub fn export(meeting: &Value, path: &Path, options: ExportOptions) -> Result<ExportStats> {
    let title = meeting["title"].as_str().unwrap_or("Meeting");
    let document_label = match options.kind {
        ExportKind::Notes => "Meeting notes",
        ExportKind::Transcript => "Transcript",
    };
    let mut writer = Writer::new(title, document_label)?;
    writer.y = 261.0;
    writer.text_at(title, 27.5, true, INK, LEFT, BODY_W, 11.0);
    writer.y -= 1.5;

    let mut metadata = Vec::new();
    if let Some(date) = meeting["started_at"].as_str() {
        metadata.push(display_date(date));
    }
    if let Some(duration) = meeting_duration(meeting) {
        metadata.push(duration);
    }
    if !metadata.is_empty() {
        writer.text(&metadata.join("  ·  "), 9.0, false, MUTED, 0.0);
    }
    let attendees = attendee_names(meeting);
    if !attendees.is_empty() {
        writer.y -= 0.3;
        writer.text(&attendees.join(", "), 9.0, false, QUIET, 0.0);
    }
    writer.y -= 7.5;

    let turns = transcript_turns(meeting);
    match options.kind {
        ExportKind::Notes => render_notes(meeting, &mut writer, options.summary_id),
        ExportKind::Transcript => render_transcript(meeting, &mut writer, &turns),
    }
    let stats = ExportStats {
        pages: writer.page,
        transcript_turns: turns.len(),
    };
    save_standard_pdf(writer.doc, path)?;
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn meeting_with_segments(count: usize) -> Value {
        let segments = (0..count)
            .map(|index| {
                let channel = if index % 2 == 0 { "me" } else { "them" };
                json!({
                    "channel": channel,
                    "speaker": if channel == "me" { Value::Null } else { json!("Jordan") },
                    "t0_ms": (index as i64) * 18_000,
                    "t1_ms": (index as i64) * 18_000 + 15_000,
                    "text": format!(
                        "This is a representative transcript turn about the launch plan, customer feedback, dependencies, and the next concrete decision number {index}."
                    )
                })
            })
            .collect::<Vec<_>>();
        json!({
            "title": "Quarterly Product Review",
            "started_at": "2026-07-13T15:00:00Z",
            "raw_notes": "Keep the launch narrow.\n\n- Confirm the pilot cohort",
            "event_json": {"attendees": [
                {"name":"Jordan Lee","email":"jordan@example.com","self":false},
                {"name":"Edison","email":"edison@example.com","self":true}
            ]},
            "summaries": [
                {"id":1,"template":"Meeting","content_md":"## Overview\nThe launch moved from a broad release to a 12-customer pilot. That makes onboarding quality, not signup volume, the first proof point; legal approval remains the gating dependency.\n\n## Decisions\n- **Narrow the launch:** Start with 12 design partners before opening self-serve access.\n- Measure activation after the first completed workflow, not account creation.\n\n## Action Items\n- [ ] Jordan - confirm the design-partner list by Friday.\n- [ ] Unassigned - resolve the data-retention language before invitations go out.\n\n## Open Questions & Risks\n- Legal review can still move the pilot date, and no fallback date was chosen."},
                {"id":2,"template":"Project Update","content_md":"## Status\nThe pilot is on track.\n\n## Risks & Blockers\n- Legal review is still open."}
            ],
            "segments": segments
        })
    }

    #[test]
    fn builds_a_grounded_readout_without_source_tokens() {
        let sections = markdown_sections(
            "## Overview\nThe pilot became the launch plan. [03:14] [notes]\n\n\
             ## Decisions\n- Keep the cohort small. [05:20]\n- Measure activation. [08:02]\n\n\
             ## Action Items\n- [ ] Jordan - confirm the list. [12:10]\n\n\
             ## Risks & Open Questions\n- Legal timing is unresolved. [16:44]",
        );
        assert_eq!(
            first_prose(&sections[0]),
            "The pilot became the launch plan."
        );
        assert_eq!(
            notes_counts(&sections),
            NotesCounts {
                decisions: 2,
                next_moves: 1,
                open_threads: 1,
            }
        );
    }

    #[test]
    fn selects_only_the_requested_summary_for_the_notes_export() {
        let meeting = meeting_with_segments(1);
        let selected = selected_summary(&meeting, Some(2)).unwrap();
        assert_eq!(selected["template"], "Project Update");
        assert_eq!(selected_summary(&meeting, Some(99)).unwrap()["id"], 1);
    }

    #[test]
    fn coalesces_consecutive_segments_from_the_same_speaker() {
        let meeting = json!({"segments": [
            {"channel":"them","speaker":"Jordan","t0_ms":0,"text":"First point."},
            {"channel":"them","speaker":"Jordan","t0_ms":1000,"text":"Second point."},
            {"channel":"me","speaker":null,"t0_ms":2000,"text":"Response."}
        ]});
        assert_eq!(
            transcript_turns(&meeting),
            vec![
                TranscriptTurn {
                    speaker: "Jordan".into(),
                    start_ms: 0,
                    text: "First point. Second point.".into()
                },
                TranscriptTurn {
                    speaker: "Me".into(),
                    start_ms: 2000,
                    text: "Response.".into()
                },
            ]
        );
    }

    #[test]
    fn notes_export_excludes_the_transcript_even_when_it_is_long() {
        let path = std::env::temp_dir().join("noted-meeting-notes-test.pdf");
        let stats = export(
            &meeting_with_segments(180),
            &path,
            ExportOptions {
                kind: ExportKind::Notes,
                summary_id: Some(1),
            },
        )
        .unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > 1_000);
        assert_eq!(
            stats.pages, 1,
            "the full transcript must not leak into the notes export"
        );
        assert_eq!(stats.transcript_turns, 180);
        let pdf = printpdf::lopdf::Document::load(&path).unwrap();
        let info_id = pdf.trailer.get(b"Info").unwrap().as_reference().unwrap();
        let info = pdf.get_object(info_id).unwrap().as_dict().unwrap();
        assert!(!info.has(b"GTS_PDFXVersion"));
        assert!(!info.has(b"Identifier"));
    }

    #[test]
    fn long_notes_export_paginates_without_becoming_a_transcript() {
        let mut meeting = meeting_with_segments(180);
        let details = (1..=72)
            .map(|number| {
                format!(
                    "- Decision {number} keeps the launch focused on a specific customer need, owner, dependency, and next step."
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        meeting["summaries"][0]["content_md"] = json!(format!(
            "## Overview\nThe team reviewed the complete launch plan.\n\n## Detailed decisions\n{details}"
        ));
        let path = std::env::temp_dir().join("noted-meeting-long-notes-test.pdf");
        let stats = export(
            &meeting,
            &path,
            ExportOptions {
                kind: ExportKind::Notes,
                summary_id: Some(1),
            },
        )
        .unwrap();
        assert!(
            stats.pages >= 2,
            "the fixture should exercise a page transition"
        );
        assert!(stats.pages <= 5, "the notes export should remain compact");
        assert_eq!(stats.transcript_turns, 180);
    }

    #[test]
    fn two_column_transcript_keeps_a_long_meeting_compact() {
        let path = std::env::temp_dir().join("noted-meeting-transcript-test.pdf");
        let stats = export(
            &meeting_with_segments(180),
            &path,
            ExportOptions {
                kind: ExportKind::Transcript,
                summary_id: None,
            },
        )
        .unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > 1_000);
        assert!(
            stats.pages <= 6,
            "180 representative turns used {} pages",
            stats.pages
        );
        assert_eq!(stats.transcript_turns, 180);
    }
}
