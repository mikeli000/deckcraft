use super::*;

/// Where an input method composes for the slide (#101), on screen: at the insertion point while
/// editing text, or over the one selected shape (typing on it starts editing). `None` when there is
/// nothing to type into.
pub fn ime_area(app: &SlideApp, xf: Xf) -> Option<Rect> {
    let st = app.session.active()?;
    if let Some(ts) = &st.selection.text {
        if ts.notes {
            return None;
        }
        if let Some((top, bot)) = deckcraft_engine::cmd::text::caret_line(st, ts) {
            return Some(Rect::from_two_pos(xf.to_screen(top), xf.to_screen(bot)).expand2(vec2(1.0, 0.0)));
        }
        let sh = st.shape(ts.shape)?;
        return Some(screen_rect(xf, &deckcraft_engine::cmd::xfrm_of(&st.doc, &st.selection, sh)));
    }
    if st.selection.shapes.len() != 1 {
        return None;
    }
    let sh = st.shape(*st.selection.shapes.first()?)?;
    if sh.is_line() {
        return None;
    }
    Some(screen_rect(xf, &deckcraft_engine::cmd::xfrm_of(&st.doc, &st.selection, sh)))
}

/// Turn the input method on for the slide while it takes typing, with its candidate window at
/// the insertion point (#101). Without this egui leaves it off, and a Korean keyboard types
/// separate jamo (ㅇ ㅏ ㄴ) instead of syllables. A focused text field sets its own.
///
/// This only records the screen rect for this pass. [`crate::SlideApp`] observes the committed
/// text, owner and cause once at the end of the pass, after every IME event and editor command
/// has settled.
pub(super) fn ime_output(app: &mut SlideApp, ui: &Ui, xf: Xf) {
    let ctx = ui.ctx();
    if ctx.memory(|m| m.focused().is_some()) || app.dialog.as_ref().is_some_and(|d| d.modal()) || app.palette.is_some() || app.backstage_open() {
        return;
    }
    let Some(r) = ime_area(app, xf) else { return };
    let to_global = ctx.layer_transform_to_global(ui.layer_id()).unwrap_or_default();
    app.ime_pending = Some(to_global * r);
}

/// Seismic LiveDocs "pick or create a variable" popup: shown right under the caret while typing
/// `[[name` (`deckcraft_engine::cmd::text::livedoc_composing`), listing existing variables whose
/// name starts with what's typed so far plus a "+ Create" entry, same gesture Enter also confirms
/// (`Session::key`'s own check) — a click here does the same select-then-`text.insertVariable`
/// as that key handler.
pub(super) fn livedoc_compose_popup(app: &mut SlideApp, ui: &Ui, xf: Xf) {
    let Some(st) = app.session.active() else { return };
    let Some(ts) = st.selection.text.clone() else { return };
    let Some((start, partial)) = deckcraft_engine::cmd::text::livedoc_composing(st, &ts) else { return };
    let Some((top, _)) = deckcraft_engine::cmd::text::caret_line(st, &ts) else { return };
    let needle = partial.to_lowercase();
    let matches: Vec<String> =
        deckcraft_livedoc::list_variables(&st.doc).into_iter().filter(|n| needle.is_empty() || n.to_lowercase().starts_with(&needle)).collect();
    let exact = matches.iter().any(|n| n.eq_ignore_ascii_case(&partial));
    let create = (!partial.trim().is_empty() && !exact).then(|| partial.clone());
    if matches.is_empty() && create.is_none() {
        return;
    }
    let anchor = xf.to_screen(top);
    let mut confirm = None;
    egui::Area::new(ui.scope_id().with("livedoc_compose")).fixed_pos(anchor + vec2(0.0, 6.0)).order(egui::Order::Foreground).show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.set_max_width(220.0);
            for name in &matches {
                if ui.button(name).clicked() {
                    confirm = Some(name.clone());
                }
            }
            if let Some(name) = &create
                && ui.button(format!("+ Create \"{name}\"")).clicked()
            {
                confirm = Some(name.clone());
            }
        });
    });
    if let Some(name) = confirm {
        let _ = app.run("text.select", json!({"anchor": [start.0, start.1], "caret": [ts.caret.0, ts.caret.1]}));
        let _ = app.run("text.insertVariable", json!({"name": name}));
    }
}
