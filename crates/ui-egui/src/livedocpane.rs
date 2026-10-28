//! Variables pane: every Seismic LiveDocs `AD_HOC` variable in the document (`text.listVariables`),
//! and a way to create a new one at the insertion point (`text.insertVariable`). See
//! `deckcraft_livedoc`'s own doc comment for what a variable is and isn't yet.
//!
//! Also hosts the Seismic ribbon tab's other two actions: "Save as Livedoc PPTX" (a plain file
//! export, see [`crate::SlideApp::export_as`]) and "Upload Live" ([`start_upload`]/
//! [`SlideApp::poll_livedoc_upload`] — a real, credentialed network call that never runs on the
//! UI thread. Desktop calls `deckcraft_livedoc::upload` directly on a background thread (see its
//! own doc comment for the four-call sequence and why it's desktop-only — no credentials on this
//! computer, nothing to run). The web build has no credentials to call Seismic with at all (a
//! public wasm bundle can't hold a secret); instead it `fetch`es a same-origin proxy
//! (`apps/deckcraft-server`, forwarded there by `Trunk.toml`'s `[[proxy]]`) that holds the real
//! credentials and does the same four-call sequence server-side.

use egui::Ui;
use serde_json::json;

use crate::SlideApp;
use crate::theme::Tokens;

/// Pane-only state (nothing here is part of the document): the name being typed for a new
/// variable, a per-name sample value shown in this pane only (never on the canvas, never saved)
/// so a user can tell variables apart while building the deck, and an in-flight "Upload Live"
/// job, if any.
#[derive(Default)]
pub struct State {
    pub new_name: String,
    /// The "dynamic table" form: the table's own name and its comma-separated column names.
    pub new_table_name: String,
    pub new_table_columns: String,
    /// The "dynamic image" form: the name to bind the selected picture to.
    pub new_image_name: String,
    pub samples: std::collections::HashMap<String, String>,
    #[cfg(not(target_arch = "wasm32"))]
    upload: Option<UploadJob>,
    #[cfg(target_arch = "wasm32")]
    web_upload: Option<WebUploadJob>,
    /// The most recent "Upload Live" result, kept so the Variables pane can show a link back to
    /// the tenant even after the toast announcing it has faded.
    last_upload: Option<LastUpload>,
}

/// What the Variables pane needs from an "Upload Live" result — the fields both the desktop
/// (`deckcraft_livedoc::upload::UploadLiveResult`) and web (`WebUploadResult`) paths produce.
struct LastUpload {
    library_url: String,
    form_warning: Option<String>,
}

#[cfg(not(target_arch = "wasm32"))]
struct UploadJob {
    rx: std::sync::mpsc::Receiver<Result<deckcraft_livedoc::upload::UploadLiveResult, String>>,
}

/// What `apps/deckcraft-server`'s `POST /livedoc/upload` sends back — the browser-side mirror of
/// `deckcraft_livedoc::upload::UploadLiveResult`, which isn't available on wasm at all (the whole
/// `upload` module is desktop-only; see this file's own doc comment for why). Not shared with
/// that type: nothing here needs the credentials/`reqwest` machinery that comes with it, only its
/// JSON shape.
#[cfg(target_arch = "wasm32")]
#[derive(serde::Deserialize)]
struct WebUploadResult {
    library_url: String,
    #[serde(default)]
    form_warning: Option<String>,
}

/// The in-flight web upload: a `wasm_bindgen_futures::spawn_local` task writes its result here
/// once the `fetch` finishes. `Rc<RefCell<_>>`, not a channel — wasm32 is single-threaded, so
/// there is no second thread to hand a `Sender` to.
#[cfg(target_arch = "wasm32")]
struct WebUploadJob {
    slot: std::rc::Rc<std::cell::RefCell<Option<Result<WebUploadResult, String>>>>,
}

/// Is a text insertion point active (the same gate `text.insertVariable` itself enforces) —
/// inserting a variable only makes sense while editing a shape's text.
fn is_editing_text(app: &SlideApp) -> bool {
    app.session.active().is_some_and(|d| d.selection.text.is_some())
}

/// The highlight color for a token run on the canvas (`canvas::overlays`): the user's chosen
/// RGB, at a fixed translucency so the text already baked into the rendered slide image stays
/// readable underneath it — same alpha convention as the built-in text-selection highlight.
pub(crate) fn highlight_color(app: &SlideApp) -> egui::Color32 {
    let [r, g, b] = app.ui.livedoc_token_color;
    egui::Color32::from_rgba_unmultiplied(r, g, b, 90)
}

/// `livedoc.variables`, decoded: the pane's whole content, by kind.
#[derive(Default)]
struct Inventory {
    scalars: Vec<String>,
    tables: Vec<(String, Vec<String>)>,
    images: Vec<String>,
}

/// One row of `livedoc.usage`: a variable and the (0-based) slides it is used on.
struct Usage {
    kind: String,
    name: String,
    slides: Vec<usize>,
}

fn inventory(app: &mut SlideApp) -> Inventory {
    let strings = |v: &serde_json::Value| -> Vec<String> {
        v.as_array().map(|a| a.iter().filter_map(|n| n.as_str().map(str::to_string)).collect()).unwrap_or_default()
    };
    let Ok(v) = app.session.execute("livedoc.variables", &json!({})) else { return Inventory::default() };
    Inventory {
        scalars: strings(&v["scalars"]),
        tables: v["tables"]
            .as_array()
            .map(|a| a.iter().map(|t| (t["name"].as_str().unwrap_or_default().to_string(), strings(&t["columns"]))).collect())
            .unwrap_or_default(),
        images: strings(&v["images"]),
    }
}

fn usage(app: &mut SlideApp) -> Vec<Usage> {
    let Ok(v) = app.session.execute("livedoc.usage", &json!({})) else { return vec![] };
    v.as_array()
        .map(|rows| {
            rows.iter()
                .map(|r| Usage {
                    kind: r["kind"].as_str().unwrap_or_default().to_string(),
                    name: r["name"].as_str().unwrap_or_default().to_string(),
                    slides: r["slides"].as_array().map(|a| a.iter().filter_map(|i| i.as_u64().map(|i| i as usize)).collect()).unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Is a picture selected, and is it already bound to an image variable? `(is_picture, is_bound)`.
fn selected_picture(app: &SlideApp) -> (bool, bool) {
    let Some(d) = app.session.active() else { return (false, false) };
    let mut shapes = d.selection.shapes.iter().filter_map(|id| d.shape(*id));
    match shapes.next() {
        Some(sh) if matches!(sh.kind, deckcraft_model::ShapeKind::Picture { .. }) => (true, deckcraft_livedoc::image_variable_name(sh).is_some()),
        _ => (false, false),
    }
}

/// A section: a rule above (but not on the first), a bold title, then `body`.
fn section(ui: &mut Ui, first: bool, title: &str, body: impl FnOnce(&mut Ui)) {
    if !first {
        ui.add_space(6.0);
        ui.separator();
    }
    ui.add_space(6.0);
    ui.label(egui::RichText::new(title).font(crate::theme::bold(13.0)));
    ui.add_space(4.0);
    body(ui);
}

/// What a click in the pane asks for, run once the pane has been drawn.
enum Click {
    GoTo(usize),
    ApplyToAll { name: String, kind: &'static str },
}

pub fn show(app: &mut SlideApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let inv = inventory(app);
    let uses = usage(app);
    let mut click: Option<Click> = None;

    ui.horizontal(|ui| {
        ui.label("Token Highlight Color");
        ui.color_edit_button_srgb(&mut app.ui.livedoc_token_color);
    });

    if inv.scalars.is_empty() && inv.tables.is_empty() && inv.images.is_empty() {
        ui.add_space(6.0);
        ui.label(egui::RichText::new("No variables in this document yet.").color(t.text_dim));
    }

    // Text variables: the list, then the box to add one.
    section(ui, false, "Text variables", |ui| {
        if inv.scalars.is_empty() {
            ui.label(egui::RichText::new("None yet.").color(t.text_dim));
        } else {
            egui::Grid::new("livedoc_variables").num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
                for name in &inv.scalars {
                    ui.label(name);
                    let sample = app.livedoc.samples.entry(name.clone()).or_default();
                    ui.add(egui::TextEdit::singleline(sample).hint_text("Value").desired_width(100.0));
                    if ui.button("All slides").on_hover_text("Add this variable to every slide that doesn't use it").clicked() {
                        click = Some(Click::ApplyToAll { name: name.clone(), kind: "text" });
                    }
                    ui.end_row();
                }
            });
        }
        ui.add_space(6.0);
        let editing = is_editing_text(app);
        ui.horizontal(|ui| {
            let l = ui.label("New variable");
            ui.add_enabled(editing, egui::TextEdit::singleline(&mut app.livedoc.new_name).hint_text("name").desired_width(110.0)).labelled_by(l.id);
            let can_insert = editing && !app.livedoc.new_name.trim().is_empty();
            if ui.add_enabled(can_insert, egui::Button::new("Insert Variable")).clicked() {
                let name = app.livedoc.new_name.trim().to_string();
                if app.run("text.insertVariable", json!({"name": name})).is_ok() {
                    app.livedoc.new_name.clear();
                }
            }
        });
        if !editing {
            ui.label(egui::RichText::new("Click into a text box to insert at the cursor.").small().color(t.text_dim));
        }
    });

    // Dynamic tables: each as a table (columns across, a sample row), then the box to add one.
    section(ui, false, "Dynamic tables", |ui| {
        for (table, columns) in &inv.tables {
            ui.label(egui::RichText::new(table).font(crate::theme::bold(12.0)));
            egui::ScrollArea::horizontal().id_salt(("livedoc_table_scroll", table)).show(ui, |ui| {
                egui::Grid::new(("livedoc_table", table)).striped(true).spacing([8.0, 4.0]).show(ui, |ui| {
                    for c in columns {
                        ui.label(egui::RichText::new(c).color(t.text_dim));
                    }
                    ui.end_row();
                    for c in columns {
                        let sample = app.livedoc.samples.entry(format!("{table}.{c}")).or_default();
                        ui.add(egui::TextEdit::singleline(sample).hint_text("Value").desired_width(70.0));
                    }
                    ui.end_row();
                });
            });
            ui.add_space(4.0);
        }
        if inv.tables.is_empty() {
            ui.label(egui::RichText::new("None yet.").color(t.text_dim));
            ui.add_space(4.0);
        }
        ui.horizontal(|ui| {
            let l = ui.label("New table");
            ui.add(egui::TextEdit::singleline(&mut app.livedoc.new_table_name).hint_text("table name").desired_width(100.0)).labelled_by(l.id);
            ui.add(egui::TextEdit::singleline(&mut app.livedoc.new_table_columns).hint_text("columns, comma separated").desired_width(150.0));
        });
        let columns: Vec<String> = app.livedoc.new_table_columns.split(',').map(str::trim).filter(|c| !c.is_empty()).map(str::to_string).collect();
        let can_table = app.session.active().is_some()
            && deckcraft_livedoc::is_plain_name(&app.livedoc.new_table_name)
            && !columns.is_empty()
            && columns.iter().all(|c| deckcraft_livedoc::is_plain_name(c));
        if ui.add_enabled(can_table, egui::Button::new("Insert Dynamic Table")).clicked() {
            let name = app.livedoc.new_table_name.trim().to_string();
            if app.run("livedoc.insertDynamicTable", json!({"name": name, "columns": columns})).is_ok() {
                app.livedoc.new_table_name.clear();
                app.livedoc.new_table_columns.clear();
            }
        }
    });

    // Dynamic images: the list, then bind the selected picture.
    section(ui, false, "Dynamic images", |ui| {
        if inv.images.is_empty() {
            ui.label(egui::RichText::new("None yet.").color(t.text_dim));
        } else {
            egui::Grid::new("livedoc_images").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                for name in &inv.images {
                    ui.label(name);
                    if ui.button("All slides").on_hover_text("Copy this picture to every slide that doesn't use it").clicked() {
                        click = Some(Click::ApplyToAll { name: name.clone(), kind: "image" });
                    }
                    ui.end_row();
                }
            });
        }
        ui.add_space(6.0);
        let (is_picture, is_bound) = selected_picture(app);
        ui.horizontal(|ui| {
            let l = ui.label("New image");
            ui.add_enabled(is_picture, egui::TextEdit::singleline(&mut app.livedoc.new_image_name).hint_text("image name").desired_width(110.0))
                .labelled_by(l.id);
            let can_bind = is_picture && deckcraft_livedoc::is_plain_name(&app.livedoc.new_image_name);
            if ui.add_enabled(can_bind, egui::Button::new("Bind Picture")).clicked() {
                let name = app.livedoc.new_image_name.trim().to_string();
                if app.run("livedoc.setImageVariable", json!({"name": name})).is_ok() {
                    app.livedoc.new_image_name.clear();
                }
            }
            if ui.add_enabled(is_bound, egui::Button::new("Unbind")).clicked() {
                let _ = app.run("livedoc.clearImageVariable", json!({}));
            }
        });
        if !is_picture {
            ui.label(egui::RichText::new("Select a picture on the slide first.").small().color(t.text_dim));
        }
    });

    // Where each variable is used; a slide number jumps there.
    section(ui, false, "Variables in slides", |ui| {
        if uses.is_empty() {
            ui.label(egui::RichText::new("Nothing used yet.").color(t.text_dim));
        }
        for u in &uses {
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(&u.name).font(crate::theme::bold(12.0)));
                ui.label(egui::RichText::new(&u.kind).small().color(t.text_dim));
                ui.label(egui::RichText::new("Slides").small().color(t.text_dim));
                for &i in &u.slides {
                    if ui.small_button((i + 1).to_string()).clicked() {
                        click = Some(Click::GoTo(i));
                    }
                }
            });
        }
    });

    // The last upload's link, last: it's about the whole deck, not any one variable.
    if let Some(last) = &app.livedoc.last_upload {
        section(ui, false, "Last upload", |ui| {
            ui.hyperlink_to("Open in Seismic", &last.library_url);
            if let Some(w) = &last.form_warning {
                ui.label(egui::RichText::new(format!("No default form yet: {w}")).small().color(t.text_dim));
            }
        });
    }

    match click {
        Some(Click::GoTo(i)) => {
            let _ = app.run("slide.go", json!({"index": i}));
        }
        Some(Click::ApplyToAll { name, kind }) => {
            let _ = app.run("livedoc.applyToAllSlides", json!({"name": name, "kind": kind}));
        }
        None => {}
    }
}

/// Seismic ribbon tab › "Upload Live": exports the active presentation with its LiveDocs
/// metadata and publishes it to a real Seismic tenant's Content Library — see
/// `deckcraft_livedoc::upload`'s own doc comment for the four-call sequence this runs and why
/// it's desktop-only. Missing credentials fail at once (nothing to run on a thread for); a
/// configured upload runs on a background thread so the window stays responsive, and
/// [`SlideApp::poll_livedoc_upload`] picks up the result.
#[cfg(not(target_arch = "wasm32"))]
pub fn start_upload(app: &mut SlideApp) {
    if app.livedoc.upload.is_some() {
        app.set_status("Still uploading…");
        return;
    }
    let Some(d) = app.session.active() else { return };
    let title = d.title();
    let creds = match deckcraft_livedoc::upload::SeismicCredentials::from_env() {
        Ok(c) => c,
        Err(e) => {
            app.notify(format!("Upload Live isn't configured on this computer: {e}"));
            return;
        }
    };
    // The real add-in-equivalent skeleton Upload Live's target tenant needs (not just a plain
    // AD_HOC variable list) — see `deckcraft_livedoc::export::export_pptx_for_live_upload`'s own
    // doc comment for why this, not `export_pptx_with_livedoc`, is the one Upload Live exports.
    let bytes = match deckcraft_livedoc::export_pptx_for_live_upload(&d.doc, &creds.tenant) {
        Ok(b) => b,
        Err(e) => {
            app.notify(format!("Couldn't export for upload: {e}"));
            return;
        }
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let filename = format!("{title}.pptx");
    let content_name = title;
    let spawned = std::thread::Builder::new().name("deckcraft-livedoc-upload".into()).spawn(move || {
        let r = deckcraft_livedoc::upload::upload_live_pptx(&creds, bytes, &filename, &content_name).map_err(|e| e.to_string());
        let _ = tx.send(r);
    });
    match spawned {
        Ok(_) => {
            app.livedoc.upload = Some(UploadJob { rx });
            app.set_status("Uploading to Seismic…");
        }
        Err(e) => app.notify(format!("Couldn't start the upload: {e}")),
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl SlideApp {
    /// Whether an "Upload Live" started by [`start_upload`] is still running — the ribbon's
    /// own gate against starting a second one, and what its button's label switches on.
    pub(crate) fn livedoc_upload_in_progress(&self) -> bool {
        self.livedoc.upload.is_some()
    }

    /// Take the background upload's result when it has arrived. Called once per frame from
    /// [`SlideApp::logic`], the same way [`SlideApp::poll_opening`] picks up a background file
    /// read.
    pub(crate) fn poll_livedoc_upload(&mut self) {
        let Some(job) = &self.livedoc.upload else { return };
        let r = match job.rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("the upload stopped unexpectedly".to_string()),
        };
        self.livedoc.upload = None;
        match r {
            Ok(result) => {
                self.livedoc.last_upload = Some(LastUpload { library_url: result.library_url.clone(), form_warning: result.form_warning.clone() });
                show_upload_result(self, result.library_url, result.form_warning);
            }
            Err(e) => self.notify(format!("Upload Live failed: {e}")),
        }
    }
}

/// The message [`SlideApp::poll_livedoc_upload`] shows after a successful upload (desktop or
/// web): a plain success toast when the default form was attached too, or a clearly-labeled
/// partial-failure notification when it wasn't — not the same "Uploaded live: …" wording with the
/// failure quietly appended in parentheses, which read as success either way.
fn show_upload_result(app: &mut SlideApp, library_url: String, form_warning: Option<String>) {
    match form_warning {
        None => app.toast = Some(crate::toast::Toast { text: format!("Uploaded live: {library_url}"), at: crate::now_ms() }),
        Some(w) => app.notify(format!("Uploaded, but couldn't attach a default form: {w} ({library_url})")),
    }
}

/// Seismic ribbon tab › "Upload Live" on the web build: exports the active presentation's
/// variables (no credentials needed for this part — `export_pptx_with_livedoc` only), then
/// `fetch`es `/livedoc/upload`, same-origin thanks to `Trunk.toml`'s `[[proxy]]` to
/// `apps/deckcraft-server`. See this file's own doc comment for why the real upload can't run in
/// the browser directly.
#[cfg(target_arch = "wasm32")]
pub fn start_upload(app: &mut SlideApp) {
    if app.livedoc.web_upload.is_some() {
        app.set_status("Still uploading…");
        return;
    }
    let Some(d) = app.session.active() else { return };
    let title = d.title();
    let bytes = match deckcraft_livedoc::export_pptx_with_livedoc(&d.doc) {
        Ok(b) => b,
        Err(e) => {
            app.notify(format!("Couldn't export for upload: {e}"));
            return;
        }
    };
    let slot = std::rc::Rc::new(std::cell::RefCell::new(None));
    app.livedoc.web_upload = Some(WebUploadJob { slot: slot.clone() });
    app.set_status("Uploading to Seismic…");
    wasm_bindgen_futures::spawn_local(async move {
        let r = web_fetch_upload(bytes, &title).await;
        *slot.borrow_mut() = Some(r);
    });
}

/// The `fetch` itself — a POST with the exported `.pptx` as the raw body, the content name as a
/// query parameter (there's no room for a JSON field alongside raw bytes in one request body).
#[cfg(target_arch = "wasm32")]
async fn web_fetch_upload(bytes: Vec<u8>, name: &str) -> Result<WebUploadResult, String> {
    use wasm_bindgen::{JsCast, JsValue};

    let js_err = |e: JsValue| format!("{e:?}");
    let opts = web_sys::RequestInit::new();
    opts.set_method("POST");
    opts.set_mode(web_sys::RequestMode::SameOrigin);
    opts.set_body(&js_sys::Uint8Array::from(bytes.as_slice()));
    let url = format!("/livedoc/upload?name={}", js_sys::encode_uri_component(name));
    let request = web_sys::Request::new_with_str_and_init(&url, &opts).map_err(js_err)?;
    let window = web_sys::window().ok_or("no window")?;
    let resp_value = wasm_bindgen_futures::JsFuture::from(window.fetch_with_request(&request)).await.map_err(js_err)?;
    let resp: web_sys::Response = resp_value.dyn_into().map_err(|_| "fetch didn't return a Response".to_string())?;
    let text_value = wasm_bindgen_futures::JsFuture::from(resp.text().map_err(js_err)?).await.map_err(js_err)?;
    let text = text_value.as_string().unwrap_or_default();
    if !resp.ok() {
        return Err(format!("{}: {text}", resp.status()));
    }
    serde_json::from_str(&text).map_err(|e| format!("couldn't parse the server's response ({e}): {text}"))
}

#[cfg(target_arch = "wasm32")]
impl SlideApp {
    /// [`SlideApp::livedoc_upload_in_progress`]'s web counterpart.
    pub(crate) fn livedoc_upload_in_progress(&self) -> bool {
        self.livedoc.web_upload.is_some()
    }

    /// [`SlideApp::poll_livedoc_upload`]'s web counterpart: picks up [`web_fetch_upload`]'s
    /// result once the browser's `fetch` has actually finished. Called once per frame, same as
    /// the desktop version.
    pub(crate) fn poll_livedoc_upload(&mut self) {
        let Some(job) = &self.livedoc.web_upload else { return };
        let Some(r) = job.slot.borrow_mut().take() else { return };
        self.livedoc.web_upload = None;
        match r {
            Ok(result) => {
                self.livedoc.last_upload = Some(LastUpload { library_url: result.library_url.clone(), form_warning: result.form_warning.clone() });
                show_upload_result(self, result.library_url, result.form_warning);
            }
            Err(e) => self.notify(format!("Upload Live failed: {e}")),
        }
    }
}

#[cfg(test)]
#[path = "livedocpane_tests.rs"]
mod livedocpane_tests;
