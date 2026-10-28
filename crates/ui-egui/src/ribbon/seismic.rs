use super::*;

/// Seismic LiveDocs add-in interop (see `deckcraft_livedoc`'s own doc comment for what's actually
/// implemented): a dedicated tab, the way a real Office add-in would get its own ribbon tab
/// rather than being folded into an existing PowerPoint one.
pub(super) fn seismic(app: &mut SlideApp, ui: &mut Ui) {
    let has_doc = app.session.active().is_some();
    group(ui, |ui| {
        if big_button(ui, Icon::Table, "Show Variable\nPanel", has_doc).on_hover_text("List and insert LiveDocs variables").clicked() {
            let _ = app.run("view.pane", json!({"pane": "variables"}));
        }
    });
    group(ui, |ui| {
        let uploading = app.livedoc_upload_in_progress();
        let label = if uploading { "Uploading…" } else { "Upload\nLivedoc" };
        if big_button(ui, Icon::Share, label, has_doc && !uploading)
            .on_hover_text("Publish this presentation's LiveDocs variables to your Seismic tenant's Content Library")
            .clicked()
        {
            crate::livedocpane::start_upload(app);
        }
        if big_button(ui, Icon::SaveAs, "Save as\nLivedoc PPTX", has_doc).on_hover_text("Export a .pptx with LiveDocs variable metadata").clicked() {
            app.export_as("livedocPptx", json!({}));
        }
    });
}
