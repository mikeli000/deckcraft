//! Interop with **Seismic LiveDocs**, a real third-party PowerPoint add-in that personalizes a
//! template from a data source. This crate knows nothing about PowerPoint internals or OOXML —
//! `deckcraft-model`/`deckcraft-pptx` stay completely ignorant of LiveDocs; this crate only
//! interprets the opaque `RunKind::Token` payload it creates (see that variant's doc comment).
//!
//! # What this covers today
//!
//! Only the simplest case: a plain-text "AD_HOC" variable, written by the real add-in as a run
//! whose entire literal text is `[[AD_HOC:name]]`. [`recognize_variables`] turns a matching `Text`
//! run (from import, or from any other source of plain text) into a `Token` run, so the editor
//! treats it as one atomic unit — same as a date/slide-number field — rather than text a user could
//! accidentally split while editing. [`display_text`]/[`token_data`] are the matching pair for
//! creating a brand-new one. [`export::export_pptx_with_livedoc`]/[`export::export_pptx_for_live_upload`]
//! write the full custom-XML structure (document identity, variable lists, per-shape
//! `Strauss.PersonalizationDefinition` bindings) a real add-in-authored file carries — see that
//! module's own doc comment for exactly what's confirmed and how the two differ; [`upload`]
//! publishes the result straight into a real Seismic tenant's Content Library ("Upload Live").
//!
//! [`recognize_variables`] also reaches into a table's cells (same `[[AD_HOC:name]]` convention,
//! named `table.column` — see [`dynamic_table_binding`]'s own doc comment for the real add-in's
//! "dynamic table" convention this matches: one row's cells, each a dot-named token sharing the
//! row's own name as a common prefix). A picture can't carry a text token (it has no text), so an
//! image variable is marked a different way: [`mark_image_variable`]/[`image_variable_name`] use
//! the shape's own `tags` (`Shape::tags`, the generic per-shape add-in-data mechanism
//! `deckcraft-model`/`deckcraft-pptx` already have for exactly this purpose) rather than a run.
//!
//! # What this deliberately does not do yet
//!
//! - Only the `AD_HOC` (plain string, dynamic-table and image) variable kinds exist. Charts, and
//!   any binding richer than "one external image" (a PDF, a whole other deck) are not modelled.

use deckcraft_model::text::{Run, RunKind, TextBody};
use deckcraft_model::{Presentation, Shape, ShapeKind};

pub mod export;
#[cfg(not(target_arch = "wasm32"))]
pub mod upload;

pub use export::{export_pptx_for_live_upload, export_pptx_with_livedoc};

/// `"[[AD_HOC:name]]"` — the real add-in's display-text convention for a plain-string variable
/// (confirmed against a real add-in-authored sample file), and what DeckCraft itself writes for a
/// variable the user creates here, so the two stay indistinguishable once exported.
pub fn display_text(name: &str) -> String {
    format!("[[AD_HOC:{name}]]")
}

/// The opaque `RunKind::Token::data` payload for a plain-string variable named `name`.
pub fn token_data(name: &str) -> String {
    format!(r#"{{"kind":"variable","name":"{}"}}"#, json_escape(name))
}

/// Minimal JSON string escaping for the one place this crate hand-builds a JSON string literal
/// outside `serde_json` (`token_data` is a plain opaque `String` on `Run`, not something this
/// crate deserializes into a struct) — variable names are realistically always a simple
/// identifier, but one containing `"`/`\` shouldn't be able to produce invalid JSON.
fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// `run.kind`'s `Token` payload, if it's one of this crate's own variables: `Some(name)` for a
/// `Token` run whose `data` decodes as `{"kind":"variable","name":"..."}`, `None` for anything
/// else (plain text, a token some *other* plugin created, or malformed/missing JSON).
pub fn variable_token_name(run: &Run) -> Option<String> {
    let RunKind::Token { data } = &run.kind else { return None };
    let value: serde_json::Value = serde_json::from_str(data).ok()?;
    if value.get("kind")?.as_str()? != "variable" {
        return None;
    }
    Some(value.get("name")?.as_str()?.to_string())
}

/// `"[[AD_HOC:my_name]]"` -> `Some("my_name")`; anything else (plain text, `"[[my_name]]"`
/// without the prefix, or a token that's only part of a larger run) -> `None`.
fn parse_display_text(text: &str) -> Option<&str> {
    text.strip_prefix("[[AD_HOC:")?.strip_suffix("]]").filter(|name| !name.is_empty())
}

/// Scans every shape's text (and, for a table, every cell's text) for a run whose *entire* literal
/// text is exactly `[[AD_HOC:name]]` and turns it into this crate's own atomic variable token
/// (`RunKind::Token` + [`token_data`]) — see this crate's own doc comment for why only the real
/// add-in's exact display-text form is recognized, and [`dynamic_table_binding`] for how a group
/// of same-row, same-prefix table tokens becomes a "dynamic table" at export time.
///
/// Meant to run once, right after import, on any presentation about to be handed to a user for
/// editing — without this, a real LiveDocs file's variables show as perfectly ordinary text:
/// splittable by an edit, and not something a future "tokens in this document" UI could list.
///
/// Returns how many tokens were recognized, for a caller that wants to log or test it; `0` for a
/// document that never had one.
pub fn recognize_variables(presentation: &mut Presentation) -> usize {
    let mut count = 0;
    for slide in &mut presentation.slides {
        let slide = std::sync::Arc::make_mut(slide);
        walk_mut(&mut slide.shapes, &mut |shape| {
            if let Some(body) = shape.text.as_mut() {
                recognize_in_body(body, &mut count);
            }
            if let ShapeKind::Table(table) = &mut shape.kind {
                for row in &mut table.rows {
                    for cell in &mut row.cells {
                        recognize_in_body(&mut cell.text, &mut count);
                    }
                }
            }
        });
    }
    count
}

/// [`recognize_variables`]'s per-`TextBody` logic, shared between a shape's own text and a table
/// cell's.
fn recognize_in_body(body: &mut TextBody, count: &mut usize) {
    for paragraph in &mut body.paragraphs {
        for run in &mut paragraph.runs {
            if !matches!(run.kind, RunKind::Text) {
                continue;
            }
            let Some(name) = parse_display_text(&run.text) else { continue };
            run.kind = RunKind::Token { data: token_data(name) };
            *count += 1;
        }
    }
}

/// The add-in tag [`mark_image_variable`]/[`image_variable_name`] use on a picture shape — see
/// this crate's own top-level doc comment for why a picture needs a different marker than a text
/// token (it has no text to turn atomic).
const IMAGE_VARIABLE_TAG: &str = "deckcraft.livedoc.image";

/// Marks `shape` (expected to be a `ShapeKind::Picture`, though this doesn't check — a caller
/// should gate on that itself, same as every other shape-kind-specific editor command) as bound to
/// an external-image variable named `name`, replacing any previous binding.
pub fn mark_image_variable(shape: &mut Shape, name: &str) {
    shape.tags.retain(|(k, _)| k != IMAGE_VARIABLE_TAG);
    shape.tags.push((IMAGE_VARIABLE_TAG.to_string(), name.to_string()));
}

/// `shape`'s image-variable name, if [`mark_image_variable`] set one on it (and it's still a
/// picture — a shape that changed kind after being marked, e.g. via a "change picture" that
/// somehow swapped the whole shape, is intentionally unbound rather than carrying a stale tag
/// forward onto content a real LiveDocs session never associated with it).
pub fn image_variable_name(shape: &Shape) -> Option<String> {
    if !matches!(shape.kind, ShapeKind::Picture { .. }) {
        return None;
    }
    shape.tags.iter().find(|(k, _)| k == IMAGE_VARIABLE_TAG).map(|(_, v)| v.clone())
}

/// One "dynamic table" binding: `table_name` is the shared prefix every cell in the bound row's
/// token is named `{table_name}.{column}` with (`zzzz` in `[[AD_HOC:zzzz.aaa]]`); `row` is that
/// row's index in `Table::rows`; `columns` is `(column name, cell index)` in left-to-right order —
/// confirmed against a real add-in-authored sample (`(New) Test_v1.0.PPTX`): the add-in's own
/// "insert dynamic table" always binds one whole row, one dot-named field per cell, all sharing the
/// table's own variable name as their common prefix.
pub struct DynamicTableBinding {
    pub table_name: String,
    pub row: usize,
    pub columns: Vec<(String, usize)>,
}

/// The first row in `table` whose every non-merged-away cell holds exactly one recognized token,
/// all of them named `{same prefix}.{something}` — see [`DynamicTableBinding`]'s own doc comment.
/// `None` for a table with no such row (an ordinary table, or one with per-cell variables that
/// don't share a row-wide prefix — not a shape this crate recognizes as a dynamic table).
pub fn dynamic_table_binding(table: &deckcraft_model::table::Table) -> Option<DynamicTableBinding> {
    'rows: for (ri, row) in table.rows.iter().enumerate() {
        let mut columns = Vec::new();
        let mut table_name: Option<String> = None;
        for (ci, cell) in row.cells.iter().enumerate() {
            if cell.is_covered() {
                continue;
            }
            let [paragraph] = cell.text.paragraphs.as_slice() else { continue 'rows };
            let [run] = paragraph.runs.as_slice() else { continue 'rows };
            let Some(name) = variable_token_name(run) else { continue 'rows };
            let Some((prefix, column)) = name.split_once('.') else { continue 'rows };
            match &table_name {
                None => table_name = Some(prefix.to_string()),
                Some(expected) if expected == prefix => {}
                Some(_) => continue 'rows,
            }
            columns.push((column.to_string(), ci));
        }
        if let Some(table_name) = table_name
            && !columns.is_empty()
        {
            return Some(DynamicTableBinding { table_name, row: ri, columns });
        }
    }
    None
}

/// Depth-first mutable shape walk (groups included) — `deckcraft_model::walk`'s shared-reference
/// counterpart, which this crate needs since recognition mutates runs in place. Not contributed to
/// `deckcraft-model` itself: nothing there needs it yet, and this crate is meant to stay a thin,
/// optional layer on top (see this crate's own doc comment).
fn walk_mut(shapes: &mut [Shape], f: &mut dyn FnMut(&mut Shape)) {
    fn rec(shapes: &mut [Shape], depth: usize, f: &mut dyn FnMut(&mut Shape)) {
        if depth > 64 {
            return;
        }
        for s in shapes {
            f(s);
            if let deckcraft_model::ShapeKind::Group { children, .. } = &mut s.kind {
                rec(children, depth + 1, f);
            }
        }
    }
    rec(shapes, 0, f);
}

/// Every distinct variable name in `presentation`, in first-seen (slide, then shape, then
/// paragraph, then run) order — the list a "tokens in this document" UI would show.
pub fn list_variables(presentation: &Presentation) -> Vec<String> {
    let mut names = Vec::new();
    for slide in &presentation.slides {
        deckcraft_model::walk(&slide.shapes, &mut |shape, _depth| {
            if let Some(body) = &shape.text {
                for_each_token_name(body, |name| push_unique(&mut names, name));
            }
            match &shape.kind {
                ShapeKind::Table(table) => {
                    for row in &table.rows {
                        for cell in &row.cells {
                            for_each_token_name(&cell.text, |name| push_unique(&mut names, name));
                        }
                    }
                }
                ShapeKind::Picture { .. } => {
                    if let Some(name) = image_variable_name(shape) {
                        push_unique(&mut names, name);
                    }
                }
                _ => {}
            }
        });
    }
    names
}

/// One dynamic table in the document, for the Variables pane: `name` is the shared prefix, `columns`
/// the leaf names left to right (see [`DynamicTableBinding`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableVariable {
    pub name: String,
    pub columns: Vec<String>,
}

/// Every variable in the document, split by kind — what the Variables pane shows, unlike
/// [`list_variables`]'s flat name list (a table's `zzzz.aaa` cells are one table here, not three
/// scalars).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VariableInventory {
    pub scalars: Vec<String>,
    pub tables: Vec<TableVariable>,
    pub images: Vec<String>,
}

/// One variable occurrence found by [`visit`].
enum Found {
    Text(String),
    Table { name: String, columns: Vec<String> },
    Image(String),
}

/// Calls `f(slide_index, found)` for every variable in `presentation`, slide by slide: a text token
/// anywhere (a dynamic table's own bound row excepted), each dynamic table once per slide it is on,
/// each tagged picture. The one walk [`inventory`] and [`usage`] share.
fn visit(presentation: &Presentation, mut f: impl FnMut(usize, Found)) {
    for (si, slide) in presentation.slides.iter().enumerate() {
        deckcraft_model::walk(&slide.shapes, &mut |shape, _depth| {
            if let Some(body) = &shape.text {
                for_each_token_name(body, |name| f(si, Found::Text(name)));
            }
            match &shape.kind {
                ShapeKind::Table(table) => {
                    let binding = dynamic_table_binding(table);
                    for (ri, row) in table.rows.iter().enumerate() {
                        if binding.as_ref().is_some_and(|b| b.row == ri) {
                            continue;
                        }
                        for cell in &row.cells {
                            for_each_token_name(&cell.text, |name| f(si, Found::Text(name)));
                        }
                    }
                    if let Some(b) = binding {
                        f(si, Found::Table { name: b.table_name, columns: b.columns.into_iter().map(|(c, _)| c).collect() });
                    }
                }
                ShapeKind::Picture { .. } => {
                    if let Some(name) = image_variable_name(shape) {
                        f(si, Found::Image(name));
                    }
                }
                _ => {}
            }
        });
    }
}

/// [`VariableInventory`] for `presentation`, each kind in first-seen order and without repeats.
pub fn inventory(presentation: &Presentation) -> VariableInventory {
    let mut inv = VariableInventory::default();
    visit(presentation, |_, found| match found {
        Found::Text(name) => push_unique(&mut inv.scalars, name),
        Found::Image(name) => push_unique(&mut inv.images, name),
        Found::Table { name, columns } => {
            if !inv.tables.iter().any(|t| t.name == name) {
                inv.tables.push(TableVariable { name, columns });
            }
        }
    });
    inv
}

/// What kind of variable a [`VariableUsage`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VariableKind {
    Text,
    Table,
    Image,
}

/// Where one variable is used: the (0-based) slides it appears on, in order and without repeats.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VariableUsage {
    pub kind: VariableKind,
    pub name: String,
    pub slides: Vec<usize>,
}

/// Every variable with the slides it is used on, in first-seen order.
pub fn usage(presentation: &Presentation) -> Vec<VariableUsage> {
    let mut out: Vec<VariableUsage> = Vec::new();
    visit(presentation, |slide, found| {
        let (kind, name) = match found {
            Found::Text(n) => (VariableKind::Text, n),
            Found::Table { name, .. } => (VariableKind::Table, name),
            Found::Image(n) => (VariableKind::Image, n),
        };
        match out.iter_mut().find(|u| u.kind == kind && u.name == name) {
            Some(u) => {
                if !u.slides.contains(&slide) {
                    u.slides.push(slide);
                }
            }
            None => out.push(VariableUsage { kind, name, slides: vec![slide] }),
        }
    });
    out
}

/// A copy of the first picture bound to image variable `name`, for putting the same binding on
/// other slides.
pub fn find_image_variable_shape(presentation: &Presentation, name: &str) -> Option<Shape> {
    let mut found = None;
    for slide in &presentation.slides {
        deckcraft_model::walk(&slide.shapes, &mut |shape, _| {
            if found.is_none() && image_variable_name(shape).as_deref() == Some(name) {
                found = Some(shape.clone());
            }
        });
    }
    found
}

/// A text body holding one atomic variable token for `name` — what a new text-variable placement
/// (typed, inserted by command, or applied to every slide) puts in a shape or table cell.
pub fn token_body(name: &str) -> TextBody {
    let mut body = TextBody::from_text(&display_text(name));
    if let Some(run) = body.paragraphs.first_mut().and_then(|p| p.runs.first_mut()) {
        run.kind = RunKind::Token { data: token_data(name) };
    }
    body
}

/// Removes `shape`'s image-variable binding, if any.
pub fn clear_image_variable(shape: &mut Shape) {
    shape.tags.retain(|(k, _)| k != IMAGE_VARIABLE_TAG);
}

/// `true` for a name usable as a variable, table or column name: non-empty, bounded, no control
/// characters, and none of `] [ .` (`]`/`[` would break the `[[AD_HOC:name]]` display form; `.`
/// is the table/column separator).
pub fn is_plain_name(name: &str) -> bool {
    !name.trim().is_empty() && name.len() <= 256 && !name.chars().any(|c| c.is_control() || matches!(c, '[' | ']' | '.'))
}

fn push_unique(names: &mut Vec<String>, name: String) {
    if !names.contains(&name) {
        names.push(name);
    }
}

/// Calls `f` with every recognized token's variable name in `body`, in run order.
fn for_each_token_name(body: &TextBody, mut f: impl FnMut(String)) {
    for paragraph in &body.paragraphs {
        for run in &paragraph.runs {
            if let Some(name) = variable_token_name(run) {
                f(name);
            }
        }
    }
}

/// Detects a variable being typed at the caret: `[[name` with the caret right after `name`, no
/// closing `]]` yet — the moment a "pick or create a variable" popup should show. `text` is a
/// paragraph's plain text (char-indexed, matching [`deckcraft_model::edit::Pos::1`]'s convention,
/// not byte-indexed); `caret` is the caret's char offset into it.
///
/// `None` outside an active `[[...`: no unmatched `[[` before the caret on this line, or a `]`,
/// `[`, whitespace or control character sits between it and the caret (already closed, nested, or
/// not a legal variable name in progress).
///
/// Returns `(start, partial)`: `start` is the char offset of the opening `[[`'s first `[` (so a
/// caller can select from there to the caret and replace the whole `[[name` with the finished
/// token); `partial` is `name` typed so far (empty right after typing `[[`).
pub fn detect_composition(text: &str, caret: usize) -> Option<(usize, String)> {
    let chars: Vec<char> = text.chars().collect();
    let caret = caret.min(chars.len());
    let mut i = caret;
    while i >= 2 {
        let c = chars[i - 1];
        if c == '[' && chars[i - 2] == '[' {
            return Some((i - 2, chars[i..caret].iter().collect()));
        }
        if c == ']' || c == '[' || c.is_whitespace() || c.is_control() {
            return None;
        }
        i -= 1;
    }
    None
}

/// [`detect_composition`], given the text body being edited and its caret's `(paragraph, char)`
/// position — the plumbing (which paragraph, what its plain text is) every caller otherwise
/// needs, shared so engine (`Enter` confirms) and UI (the popup) don't each duplicate it.
pub fn composing_at(body: &deckcraft_model::text::TextBody, caret: (usize, usize)) -> Option<((usize, usize), String)> {
    let para = body.paragraphs.get(caret.0)?;
    let (start, partial) = detect_composition(&para.text(), caret.1)?;
    Some(((caret.0, start), partial))
}

/// Every `RunKind::Token` run's `(paragraph, start_char, end_char)` span in `body` — what a
/// renderer highlighting live variables on the canvas needs. Not specific to this crate's own
/// `AD_HOC` tokens: any plugin's atomic token counts, since the visual "this is a live field, not
/// plain text" cue applies the same way regardless of which plugin owns it.
pub fn token_ranges(body: &deckcraft_model::text::TextBody) -> Vec<(usize, usize, usize)> {
    let mut out = Vec::new();
    for (pi, para) in body.paragraphs.iter().enumerate() {
        let mut off = 0;
        for run in &para.runs {
            let len = run.text.chars().count();
            if matches!(run.kind, RunKind::Token { .. }) {
                out.push((pi, off, off + len));
            }
            off += len;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use deckcraft_model::text::{Paragraph, TextBody};
    use deckcraft_model::{Slide, defaults};

    /// A presentation with exactly one slide (`defaults::new_presentation`'s own title slide
    /// replaced, not added to, so it's always at index 0) holding one shape whose text is a
    /// single run of `text`.
    fn doc_with_text(text: &str) -> Presentation {
        let mut p = defaults::new_presentation(None);
        let shape = Shape { text: Some(TextBody::from_text(text)), ..Default::default() };
        p.slides = vec![std::sync::Arc::new(Slide { shapes: vec![shape], ..Default::default() })];
        p
    }

    #[test]
    fn parse_display_text_recognizes_only_the_real_prefixed_form() {
        assert_eq!(parse_display_text("[[AD_HOC:my_name]]"), Some("my_name"));
        assert_eq!(parse_display_text("[[my_name]]"), None);
        assert_eq!(parse_display_text("hello [[AD_HOC:my_name]] world"), None);
        assert_eq!(parse_display_text("plain text"), None);
        assert_eq!(parse_display_text("[[AD_HOC:]]"), None, "an empty name is not a variable");
    }

    #[test]
    fn recognize_variables_marks_a_matching_run_as_an_atomic_token_and_leaves_its_text_untouched() {
        let mut p = doc_with_text("[[AD_HOC:my_name]]");
        assert_eq!(recognize_variables(&mut p), 1);
        let run = &p.slides[0].shapes[0].text.as_ref().unwrap().paragraphs[0].runs[0];
        assert_eq!(run.text, "[[AD_HOC:my_name]]");
        assert_eq!(variable_token_name(run), Some("my_name".to_string()));
    }

    #[test]
    fn recognize_variables_ignores_plain_text_and_non_matching_brackets() {
        let mut p = doc_with_text("[[my_name]]");
        assert_eq!(recognize_variables(&mut p), 0);
        assert!(matches!(p.slides[0].shapes[0].text.as_ref().unwrap().paragraphs[0].runs[0].kind, RunKind::Text));
    }

    #[test]
    fn recognize_variables_does_not_recognize_a_token_already_recognized() {
        let mut p = doc_with_text("[[AD_HOC:my_name]]");
        assert_eq!(recognize_variables(&mut p), 1);
        // Running it again on an already-recognized document is a no-op, not a second count —
        // `recognize_variables` only looks at `RunKind::Text` runs.
        assert_eq!(recognize_variables(&mut p), 0);
    }

    #[test]
    fn recognize_variables_reaches_into_groups() {
        let mut p = defaults::new_presentation(None);
        let inner = Shape { text: Some(TextBody::from_text("[[AD_HOC:grouped]]")), ..Default::default() };
        let group = Shape { kind: deckcraft_model::ShapeKind::Group { children: vec![inner], child: Default::default() }, ..Default::default() };
        p.slides.push(std::sync::Arc::new(Slide { shapes: vec![group], ..Default::default() }));
        assert_eq!(recognize_variables(&mut p), 1);
    }

    #[test]
    fn variable_token_name_only_reads_this_crate_s_own_variable_tokens() {
        let mut run = Run::new("x");
        run.kind = RunKind::Token { data: token_data("foo") };
        assert_eq!(variable_token_name(&run), Some("foo".to_string()));

        run.kind = RunKind::Text;
        assert_eq!(variable_token_name(&run), None, "not a token at all");

        run.kind = RunKind::Token { data: r#"{"kind":"something_else","name":"foo"}"#.to_string() };
        assert_eq!(variable_token_name(&run), None, "a token, but not this crate's own kind");

        run.kind = RunKind::Token { data: "not json".to_string() };
        assert_eq!(variable_token_name(&run), None, "malformed payload");
    }

    #[test]
    fn list_variables_collects_distinct_names_in_first_seen_order() {
        let mut p = doc_with_text("[[AD_HOC:b]]");
        recognize_variables(&mut p);
        let shape2 = Shape {
            text: Some(TextBody {
                paragraphs: vec![Paragraph { runs: vec![Run::new("[[AD_HOC:a]]"), Run::new("[[AD_HOC:b]]")], ..Default::default() }],
                ..Default::default()
            }),
            ..Default::default()
        };
        std::sync::Arc::make_mut(&mut p.slides[0]).shapes.push(shape2);
        recognize_variables(&mut p);
        assert_eq!(list_variables(&p), vec!["b".to_string(), "a".to_string()]);
    }

    #[test]
    fn display_text_and_token_data_round_trip_through_variable_token_name() {
        let text = display_text("my_name");
        assert_eq!(text, "[[AD_HOC:my_name]]");
        let mut p = doc_with_text(&text);
        assert_eq!(recognize_variables(&mut p), 1);
        let run = &p.slides[0].shapes[0].text.as_ref().unwrap().paragraphs[0].runs[0];
        assert_eq!(variable_token_name(run), Some("my_name".to_string()));
    }

    #[test]
    fn token_data_escapes_quotes_and_backslashes_in_the_name() {
        let data = token_data(r#"a"b\c"#);
        let value: serde_json::Value = serde_json::from_str(&data).expect("should be valid JSON");
        assert_eq!(value["name"], r#"a"b\c"#);
    }

    #[test]
    fn detect_composition_finds_the_innermost_unclosed_double_bracket() {
        assert_eq!(detect_composition("[[sdf", 5), Some((0, "sdf".to_string())));
        assert_eq!(detect_composition("[[", 2), Some((0, String::new())), "right after typing [[, name is empty so far");
        assert_eq!(detect_composition("hi [[na", 7), Some((3, "na".to_string())), "leading text before the [[ is irrelevant");
        assert_eq!(detect_composition("a[[b[[c", 7), Some((4, "c".to_string())), "the most recent [[, not an earlier one");
    }

    #[test]
    fn detect_composition_is_none_outside_an_active_double_bracket() {
        assert_eq!(detect_composition("plain text", 5), None, "no [[ at all");
        assert_eq!(detect_composition("[[AD_HOC:sdf]]", 13), None, "a ']' sits between the [[ and the caret");
        assert_eq!(detect_composition("[[na me", 7), None, "a space breaks the name being typed");
        assert_eq!(detect_composition("[", 1), None, "only one '[' so far");
        assert_eq!(detect_composition("", 0), None);
    }

    #[test]
    fn composing_at_locates_the_paragraph_and_delegates_to_detect_composition() {
        let body = TextBody { paragraphs: vec![Paragraph::new("first"), Paragraph::new("[[sdf")], ..Default::default() };
        assert_eq!(composing_at(&body, (1, 5)), Some(((1, 0), "sdf".to_string())));
        assert_eq!(composing_at(&body, (0, 5)), None, "paragraph 0 has no open [[");
        assert_eq!(composing_at(&body, (9, 0)), None, "no paragraph 9");
    }

    #[test]
    fn token_ranges_finds_every_token_run_s_char_span() {
        // "a " (2) + "[[AD_HOC:b]]" (12, a token) + " c " (3) + "[[AD_HOC:d]]" (12, a token).
        let runs = vec![
            Run::new("a "),
            Run { text: display_text("b"), kind: RunKind::Token { data: token_data("b") }, ..Run::new("") },
            Run::new(" c "),
            Run { text: display_text("d"), kind: RunKind::Token { data: token_data("d") }, ..Run::new("") },
        ];
        let body = TextBody { paragraphs: vec![Paragraph { runs, ..Default::default() }], ..Default::default() };
        assert_eq!(token_ranges(&body), vec![(0, 2, 14), (0, 17, 29)]);
    }

    #[test]
    fn token_ranges_is_empty_without_any_token_runs() {
        let body = TextBody::from_text("plain text, no variables");
        assert!(token_ranges(&body).is_empty());
    }

    fn cell_with_text(text: &str) -> deckcraft_model::table::Cell {
        deckcraft_model::table::Cell::new(text)
    }

    /// A 2-row, 3-column table: row 0 is plain header text, row 1 is `[[AD_HOC:zzzz.col]]` tokens
    /// — the shape `recognize_variables`/`dynamic_table_binding` should treat as a dynamic table
    /// bound to `zzzz`, matching a real add-in-authored sample's own convention.
    fn dynamic_table() -> deckcraft_model::table::Table {
        deckcraft_model::table::Table {
            rows: vec![
                deckcraft_model::table::Row { height: 20.0, cells: vec![cell_with_text("aaa"), cell_with_text("bbb"), cell_with_text("ccc")] },
                deckcraft_model::table::Row {
                    height: 20.0,
                    cells: vec![cell_with_text("[[AD_HOC:zzzz.aaa]]"), cell_with_text("[[AD_HOC:zzzz.bbb]]"), cell_with_text("[[AD_HOC:zzzz.ccc]]")],
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn recognize_variables_reaches_into_table_cells() {
        let mut p = defaults::new_presentation(None);
        let shape = Shape { kind: deckcraft_model::ShapeKind::Table(dynamic_table()), ..Default::default() };
        p.slides.push(std::sync::Arc::new(Slide { shapes: vec![shape], ..Default::default() }));
        assert_eq!(recognize_variables(&mut p), 3, "one token per bound cell, the header row is plain text");
        let ShapeKind::Table(table) = &p.slides[1].shapes[0].kind else { panic!("still a table") };
        let run = &table.rows[1].cells[0].text.paragraphs[0].runs[0];
        assert_eq!(variable_token_name(run), Some("zzzz.aaa".to_string()));
        let header_run = &table.rows[0].cells[0].text.paragraphs[0].runs[0];
        assert!(matches!(header_run.kind, RunKind::Text), "the header row isn't a token");
    }

    #[test]
    fn dynamic_table_binding_finds_the_bound_row_and_its_columns() {
        let mut table = dynamic_table();
        recognize_cells(&mut table);
        let binding = dynamic_table_binding(&table).expect("row 1 is a dynamic-table binding");
        assert_eq!(binding.table_name, "zzzz");
        assert_eq!(binding.row, 1);
        assert_eq!(binding.columns, vec![("aaa".to_string(), 0), ("bbb".to_string(), 1), ("ccc".to_string(), 2)]);
    }

    #[test]
    fn dynamic_table_binding_is_none_for_an_ordinary_table() {
        let mut table = deckcraft_model::table::Table::new(2, 2, 200.0, 20.0);
        recognize_cells(&mut table);
        assert!(dynamic_table_binding(&table).is_none());
    }

    #[test]
    fn dynamic_table_binding_ignores_a_row_whose_cells_dont_share_one_prefix() {
        let mut table = deckcraft_model::table::Table {
            rows: vec![deckcraft_model::table::Row {
                height: 20.0,
                cells: vec![cell_with_text("[[AD_HOC:zzzz.aaa]]"), cell_with_text("[[AD_HOC:other.bbb]]")],
            }],
            ..Default::default()
        };
        recognize_cells(&mut table);
        assert!(dynamic_table_binding(&table).is_none(), "mismatched prefixes aren't one dynamic table");
    }

    /// Test-only: runs [`recognize_variables`]'s per-body logic directly on a table's own cells,
    /// without needing a whole `Presentation`/`Slide` wrapper.
    fn recognize_cells(table: &mut deckcraft_model::table::Table) {
        let mut count = 0;
        for row in &mut table.rows {
            for cell in &mut row.cells {
                recognize_in_body(&mut cell.text, &mut count);
            }
        }
    }

    #[test]
    fn image_variable_name_round_trips_through_mark_image_variable() {
        let mut shape = Shape { kind: deckcraft_model::ShapeKind::Picture { fill: Default::default() }, ..Default::default() };
        assert_eq!(image_variable_name(&shape), None);
        mark_image_variable(&mut shape, "my_image");
        assert_eq!(image_variable_name(&shape), Some("my_image".to_string()));
        // Re-marking replaces, rather than accumulating, the tag.
        mark_image_variable(&mut shape, "other_image");
        assert_eq!(image_variable_name(&shape), Some("other_image".to_string()));
        assert_eq!(shape.tags.len(), 1);
    }

    #[test]
    fn image_variable_name_is_none_on_a_non_picture_shape() {
        let mut shape = Shape::default();
        mark_image_variable(&mut shape, "my_image");
        assert_eq!(image_variable_name(&shape), None, "the tag is there, but this isn't a picture");
    }

    #[test]
    fn list_variables_includes_table_and_image_variables() {
        let mut p = defaults::new_presentation(None);
        let table_shape = Shape { kind: deckcraft_model::ShapeKind::Table(dynamic_table()), ..Default::default() };
        let mut picture = Shape { kind: deckcraft_model::ShapeKind::Picture { fill: Default::default() }, ..Default::default() };
        mark_image_variable(&mut picture, "my_image");
        p.slides.push(std::sync::Arc::new(Slide { shapes: vec![table_shape, picture], ..Default::default() }));
        recognize_variables(&mut p);
        let names = list_variables(&p);
        assert!(names.contains(&"zzzz.aaa".to_string()), "{names:?}");
        assert!(names.contains(&"zzzz.bbb".to_string()), "{names:?}");
        assert!(names.contains(&"zzzz.ccc".to_string()), "{names:?}");
        assert!(names.contains(&"my_image".to_string()), "{names:?}");
    }

    #[test]
    fn inventory_splits_scalars_tables_and_images() {
        let mut p = defaults::new_presentation(None);
        let text = Shape { text: Some(TextBody::from_text("[[AD_HOC:my_name]]")), ..Default::default() };
        let table = Shape { kind: deckcraft_model::ShapeKind::Table(dynamic_table()), ..Default::default() };
        let mut pic = Shape { kind: deckcraft_model::ShapeKind::Picture { fill: Default::default() }, ..Default::default() };
        mark_image_variable(&mut pic, "logo");
        p.slides.push(std::sync::Arc::new(Slide { shapes: vec![text, table, pic], ..Default::default() }));
        recognize_variables(&mut p);
        let inv = inventory(&p);
        assert_eq!(inv.scalars, vec!["my_name".to_string()], "table cells aren't scalars");
        assert_eq!(inv.tables, vec![TableVariable { name: "zzzz".into(), columns: vec!["aaa".into(), "bbb".into(), "ccc".into()] }]);
        assert_eq!(inv.images, vec!["logo".to_string()]);
    }

    #[test]
    fn is_plain_name_rejects_separators_and_brackets() {
        assert!(is_plain_name("my_name"));
        for bad in [
            "", "  ", "a.b", "a]", "[a", "a
b",
        ] {
            assert!(!is_plain_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn usage_reports_each_variable_with_the_slides_it_is_on() {
        let mut p = defaults::new_presentation(None);
        let on = |text: &str| Shape { text: Some(TextBody::from_text(text)), ..Default::default() };
        p.slides = vec![
            std::sync::Arc::new(Slide { shapes: vec![on("[[AD_HOC:a]]")], ..Default::default() }),
            std::sync::Arc::new(Slide { shapes: vec![on("plain")], ..Default::default() }),
            std::sync::Arc::new(Slide { shapes: vec![on("[[AD_HOC:a]]"), on("[[AD_HOC:b]]")], ..Default::default() }),
        ];
        let table = Shape { kind: deckcraft_model::ShapeKind::Table(dynamic_table()), ..Default::default() };
        std::sync::Arc::make_mut(&mut p.slides[1]).shapes.push(table);
        recognize_variables(&mut p);
        let u = usage(&p);
        let find = |k: VariableKind, n: &str| u.iter().find(|x| x.kind == k && x.name == n).map(|x| x.slides.clone());
        assert_eq!(find(VariableKind::Text, "a"), Some(vec![0, 2]));
        assert_eq!(find(VariableKind::Text, "b"), Some(vec![2]));
        assert_eq!(find(VariableKind::Table, "zzzz"), Some(vec![1]));
        assert_eq!(find(VariableKind::Text, "zzzz.aaa"), None, "a table's cells aren't text variables");
    }
}
