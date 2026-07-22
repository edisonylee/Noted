use std::{fs::File, io::BufWriter, path::Path};

use anyhow::Result;
use printpdf::{BuiltinFont, Color, Mm, PdfDocument, PdfDocumentReference, PdfLayerReference, Rgb};
use serde_json::Value;

const PAGE_H: f32 = 297.0;
const LEFT: f32 = 22.0;
const TOP: f32 = 270.0;
const BOTTOM: f32 = 22.0;

fn clean(s: &str) -> String {
    s.replace("**", "")
        .replace("- [ ] ", "□ ")
        .replace("- [x] ", "■ ")
        .replace('—', "-")
        .replace('–', "-")
        .replace('“', "\"")
        .replace('”', "\"")
        .replace('’', "'")
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

struct Writer {
    doc: PdfDocumentReference,
    layer: PdfLayerReference,
    regular: printpdf::IndirectFontRef,
    bold: printpdf::IndirectFontRef,
    y: f32,
    page: usize,
    title: String,
}

impl Writer {
    fn new(title: &str) -> Result<Self> {
        let (doc, page, layer) = PdfDocument::new(title, Mm(210.0), Mm(PAGE_H), "Meeting notes");
        let regular = doc.add_builtin_font(BuiltinFont::Helvetica)?;
        let bold = doc.add_builtin_font(BuiltinFont::HelveticaBold)?;
        let mut out = Self {
            layer: doc.get_page(page).get_layer(layer),
            doc,
            regular,
            bold,
            y: TOP,
            page: 1,
            title: clean(title),
        };
        out.page_header();
        Ok(out)
    }

    fn page_header(&mut self) {
        self.layer
            .set_fill_color(Color::Rgb(Rgb::new(0.64, 0.35, 0.22, None)));
        self.layer.use_text(
            "NOTED  /  MEETING NOTES",
            8.5,
            Mm(LEFT),
            Mm(282.0),
            &self.bold,
        );
        self.layer
            .set_fill_color(Color::Rgb(Rgb::new(0.47, 0.43, 0.38, None)));
        self.layer.use_text(
            format!("{}  ·  {}", self.title, self.page),
            8.0,
            Mm(LEFT),
            Mm(13.0),
            &self.regular,
        );
        self.layer
            .set_fill_color(Color::Rgb(Rgb::new(0.14, 0.12, 0.10, None)));
    }

    fn next_page(&mut self) {
        let (page, layer) = self.doc.add_page(Mm(210.0), Mm(PAGE_H), "Meeting notes");
        self.layer = self.doc.get_page(page).get_layer(layer);
        self.page += 1;
        self.y = TOP;
        self.page_header();
    }

    fn ensure(&mut self, need: f32) {
        if self.y - need < BOTTOM {
            self.next_page();
        }
    }

    fn text(&mut self, text: &str, size: f32, bold: bool, color: (f32, f32, f32), indent: f32) {
        let chars = ((166.0 - indent) / (size * 0.19)).max(25.0) as usize;
        let leading = size * 0.43;
        let lines = wrap(text, chars);
        self.layer
            .set_fill_color(Color::Rgb(Rgb::new(color.0, color.1, color.2, None)));
        for line in lines {
            self.ensure(leading + 1.0);
            self.layer
                .set_fill_color(Color::Rgb(Rgb::new(color.0, color.1, color.2, None)));
            let font = if bold { &self.bold } else { &self.regular };
            self.layer
                .use_text(line, size, Mm(LEFT + indent), Mm(self.y), font);
            self.y -= leading;
        }
        self.layer
            .set_fill_color(Color::Rgb(Rgb::new(0.14, 0.12, 0.10, None)));
    }

    fn section(&mut self, name: &str) {
        self.ensure(14.0);
        self.y -= 4.0;
        self.text(&name.to_uppercase(), 14.0, true, (0.55, 0.30, 0.18), 0.0);
        self.y -= 2.5;
    }

    fn markdown(&mut self, md: &str) {
        for raw in md.lines() {
            let line = raw.trim();
            if line.is_empty() {
                self.y -= 2.5;
                continue;
            }
            if let Some(h) = line.strip_prefix("## ") {
                self.ensure(11.0);
                self.y -= 2.0;
                self.text(&h.to_uppercase(), 9.0, true, (0.46, 0.38, 0.32), 0.0);
            } else if let Some(item) = line.strip_prefix("- ") {
                self.text(&format!("• {item}"), 10.2, false, (0.14, 0.12, 0.10), 4.0);
                self.y -= 1.2;
            } else {
                self.text(line, 10.5, false, (0.14, 0.12, 0.10), 0.0);
                self.y -= 1.5;
            }
        }
    }
}

pub fn export(meeting: &Value, path: &Path) -> Result<()> {
    let title = meeting["title"].as_str().unwrap_or("Meeting");
    let mut w = Writer::new(title)?;
    w.text(title, 26.0, true, (0.14, 0.12, 0.10), 0.0);
    if let Some(date) = meeting["started_at"].as_str() {
        w.text(
            &date.replace('T', " ").chars().take(16).collect::<String>(),
            9.5,
            false,
            (0.47, 0.43, 0.38),
            0.0,
        );
    }
    w.y -= 8.0;

    if let Some(summaries) = meeting["summaries"].as_array() {
        for summary in summaries {
            w.section(summary["template"].as_str().unwrap_or("Summary"));
            w.markdown(summary["content_md"].as_str().unwrap_or(""));
        }
    }
    if let Some(notes) = meeting["raw_notes"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
    {
        w.section("Notes");
        w.markdown(notes);
    }
    if let Some(segments) = meeting["segments"].as_array().filter(|s| !s.is_empty()) {
        w.section("Transcript");
        for seg in segments {
            let who = if seg["channel"] == "me" {
                "Me"
            } else {
                seg["speaker"].as_str().unwrap_or("Them")
            };
            let secs = seg["t0_ms"].as_i64().unwrap_or(0) / 1000;
            w.text(
                &format!("{who}  {:02}:{:02}", secs / 60, secs % 60),
                8.5,
                true,
                (0.55, 0.30, 0.18),
                0.0,
            );
            w.text(
                seg["text"].as_str().unwrap_or(""),
                9.5,
                false,
                (0.14, 0.12, 0.10),
                4.0,
            );
            w.y -= 2.0;
        }
    }
    w.doc.save(&mut BufWriter::new(File::create(path)?))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn renders_meeting_pdf() {
        let path = std::env::temp_dir().join("noted-meeting-export-test.pdf");
        let meeting = json!({
            "title": "Daily Stand Up",
            "started_at": "2026-07-13T15:00:00Z",
            "raw_notes": "Follow up with the design team.",
            "summaries": [{"template":"Standup","content_md":"## Progress\n- Shipped the export flow\n\n## Next\n- Verify the PDF"}],
            "segments": [{"channel":"me","speaker":null,"t0_ms":12000,"text":"The export is ready for review."}]
        });
        export(&meeting, &path).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > 1_000);
    }

    #[test]
    #[ignore]
    fn renders_live_meeting_pdf() {
        let db = std::env::var("NOTED_TEST_DB").unwrap();
        let id: i64 = std::env::var("NOTED_TEST_MEETING")
            .unwrap()
            .parse()
            .unwrap();
        let conn = rusqlite::Connection::open(db).unwrap();
        let meeting = crate::meeting::store::get_meeting(&conn, id).unwrap();
        export(
            &meeting,
            &std::env::temp_dir().join("noted-live-meeting-export.pdf"),
        )
        .unwrap();
    }
}
