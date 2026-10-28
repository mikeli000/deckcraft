//! Seismic LiveDocs variables beyond a text token: the full variable inventory, dynamic tables and
//! image variables (see `deckcraft_livedoc`'s own doc comment). A plain text variable is
//! `text.insertVariable`.

use deckcraft_model::table::{Cell, Table};
use std::sync::Arc;

use deckcraft_livedoc::VariableKind;
use deckcraft_model::{Shape, ShapeId, ShapeKind};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "livedoc.variables",
            "LiveDocs Variables",
            [],
            None,
            "{} → {scalars: [name], tables: [{name, columns: [name]}], images: [name]} — every LiveDocs variable, by kind",
            has_doc,
            variables
        ),
        cmd!(
            query "livedoc.usage",
            "LiveDocs Variables by Slide",
            [],
            None,
            "{} → [{kind: text|table|image, name, slides: [index]}] — where each variable is used (0-based slide indices)",
            has_doc,
            usage
        ),
        cmd!(
            "livedoc.applyToAllSlides",
            "Apply Variable to All Slides",
            [],
            None,
            "{name, kind: text|image} → {added} — puts the variable on every slide that lacks it (a text box with its token, or a copy of its picture)",
            has_slide,
            apply_to_all
        ),
        cmd!(
            "livedoc.insertDynamicTable",
            "Insert Dynamic Table",
            [],
            None,
            "{name, columns: [name]} — a header row plus one `[[AD_HOC:name.column]]` row per column, as a new table on the slide",
            has_slide,
            insert_table
        ),
        cmd!(
            "livedoc.setImageVariable",
            "Set Image Variable",
            [],
            None,
            "{name, id?} — binds the selected picture to a LiveDocs image variable",
            has_selection,
            set_image
        ),
        cmd!(
            "livedoc.clearImageVariable",
            "Clear Image Variable",
            [],
            None,
            "{id?} — unbinds the selected picture from its LiveDocs image variable",
            has_selection,
            clear_image
        ),
    ]
}

fn variables(s: &mut Session, _: &Value) -> Result<Value> {
    let inv = deckcraft_livedoc::inventory(&s.doc()?.doc);
    let tables: Vec<Value> = inv.tables.iter().map(|t| json!({"name": t.name, "columns": t.columns})).collect();
    Ok(json!({"scalars": inv.scalars, "tables": tables, "images": inv.images}))
}

fn insert_table(s: &mut Session, p: &Value) -> Result<Value> {
    let name = str_param(p, "name").map(str::trim).unwrap_or_default();
    if !deckcraft_livedoc::is_plain_name(name) {
        return Err(bad("livedoc.insertDynamicTable", "`name` must be a plain name (no `.`, `[` or `]`)"));
    }
    let columns: Vec<String> = p
        .get("columns")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|c| c.as_str().map(|c| c.trim().to_string())).collect())
        .unwrap_or_default();
    if columns.is_empty() || columns.len() > 20 {
        return Err(bad("livedoc.insertDynamicTable", "give 1 to 20 `columns`"));
    }
    if let Some(c) = columns.iter().find(|c| !deckcraft_livedoc::is_plain_name(c)) {
        return Err(bad("livedoc.insertDynamicTable", format!("`{c}` is not a plain column name (no `.`, `[` or `]`)")));
    }
    if columns.iter().enumerate().any(|(i, c)| columns[..i].contains(c)) {
        return Err(bad("livedoc.insertDynamicTable", "column names must be distinct"));
    }

    let size = s.doc()?.doc.slide_size;
    let (rows, rh) = (2usize, 40.0);
    let rect = deckcraft_geom::Xfrm::new(size.width * 0.1, (size.height - rh * rows as f64).max(0.0) / 2.0, size.width * 0.8, rh * rows as f64);
    let mut t = Table::new(rows, columns.len(), rect.w, rh);
    for (ci, col) in columns.iter().enumerate() {
        if let Some(cell) = t.cell_mut(0, ci) {
            *cell = Cell::new(col);
        }
        if let Some(cell) = t.cell_mut(1, ci) {
            cell.text = deckcraft_livedoc::token_body(&format!("{name}.{col}"));
        }
    }
    let id = insert::add_shape(s, "livedoc.insertDynamicTable", Shape { xfrm: Some(rect), kind: ShapeKind::Table(t), ..Default::default() }, true)?;
    Ok(json!({"id": id}))
}

fn set_image(s: &mut Session, p: &Value) -> Result<Value> {
    let name = str_param(p, "name").map(str::trim).unwrap_or_default().to_string();
    if !deckcraft_livedoc::is_plain_name(&name) {
        return Err(bad("livedoc.setImageVariable", "`name` must be a plain name (no `.`, `[` or `]`)"));
    }
    edit_shapes(s, p, "livedoc.setImageVariable", |sh| {
        if !matches!(sh.kind, ShapeKind::Picture { .. }) {
            return Err(bad("livedoc.setImageVariable", "select a picture"));
        }
        deckcraft_livedoc::mark_image_variable(sh, &name);
        Ok(())
    })?;
    ok()
}

fn clear_image(s: &mut Session, p: &Value) -> Result<Value> {
    edit_shapes(s, p, "livedoc.clearImageVariable", |sh| {
        deckcraft_livedoc::clear_image_variable(sh);
        Ok(())
    })?;
    ok()
}

fn usage(s: &mut Session, _: &Value) -> Result<Value> {
    let rows: Vec<Value> = deckcraft_livedoc::usage(&s.doc()?.doc)
        .into_iter()
        .map(|u| {
            let kind = match u.kind {
                VariableKind::Text => "text",
                VariableKind::Table => "table",
                VariableKind::Image => "image",
            };
            json!({"kind": kind, "name": u.name, "slides": u.slides})
        })
        .collect();
    Ok(Value::Array(rows))
}

/// `livedoc.applyToAllSlides`: a text variable becomes a text box with its token, an image variable
/// a copy of its picture, on every slide that doesn't already use it. One undo step.
fn apply_to_all(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "livedoc.applyToAllSlides";
    let name = str_param(p, "name").map(str::trim).unwrap_or_default().to_string();
    let kind = match str_param(p, "kind") {
        Some("text") => VariableKind::Text,
        Some("image") => VariableKind::Image,
        _ => return Err(bad(CMD, "`kind` must be `text` or `image`")),
    };
    let doc = &s.doc()?.doc;
    let used = deckcraft_livedoc::usage(doc)
        .into_iter()
        .find(|u| u.kind == kind && u.name == name)
        .ok_or_else(|| bad(CMD, format!("no such variable `{name}`")))?
        .slides;
    let picture = match kind {
        VariableKind::Image => Some(deckcraft_livedoc::find_image_variable_shape(doc, &name).ok_or_else(|| bad(CMD, "its picture is gone"))?),
        _ => None,
    };
    let size = doc.slide_size;
    let missing: Vec<usize> = (0..doc.slides.len()).filter(|i| !used.contains(i)).collect();
    if missing.is_empty() {
        return Ok(json!({"added": 0}));
    }
    let added = s.edit(|doc, _sel| {
        let mut added = 0usize;
        for &i in &missing {
            let id = ShapeId(doc.alloc_id().ok_or_else(|| bad(CMD, NO_ID_SPACE))?);
            let mut shape = match &picture {
                Some(pic) => Shape { custom_xml_ref: None, ..pic.clone() },
                None => {
                    let rect = deckcraft_geom::Xfrm::new(size.width * 0.1, size.height * 0.8, size.width * 0.8, 40.0);
                    let mut b = insert::new_text_box(rect, "");
                    b.text = Some(deckcraft_livedoc::token_body(&name));
                    b
                }
            };
            shape.id = id;
            let slide = doc.slides.get_mut(i).ok_or_else(|| bad(CMD, "no such slide"))?;
            Arc::make_mut(slide).shapes.push(shape);
            added += 1;
        }
        Ok(added)
    })?;
    Ok(json!({"added": added}))
}
