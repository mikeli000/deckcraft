//! Variables pane driven like a person: type a name, click Insert, see it listed.

use deckcraft_engine::Session;
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

use crate::{Services, SlideApp};

fn harness_with_a_shape_being_edited() -> (Harness<'static, SlideApp>, u64) {
    let mut s = Session::with_new();
    let id = s.execute("insert.textBox", &json!({"rect": [100, 100, 500, 50]})).unwrap()["id"].as_u64().unwrap();
    s.execute("text.edit", &json!({"id": id})).unwrap();
    s.execute("text.insert", &json!({"text": "Hello "})).unwrap();
    let mut app = SlideApp::new(s, Services::default());
    app.ui.pane = Some("variables".into());
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut SlideApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    (h, id)
}

#[test]
fn typing_a_name_and_clicking_insert_adds_an_atomic_token() {
    let (mut h, id) = harness_with_a_shape_being_edited();
    assert!(h.query_by_label("No variables in this document yet.").is_some());

    h.get_by_role_and_label(Role::TextInput, "New variable").click();
    h.get_by_role_and_label(Role::TextInput, "New variable").type_text("my_name");
    h.run_steps(3);
    h.get_by_label("Insert Variable").click();
    h.run_steps(3);

    let text = h.state().session.active().unwrap().shapes().iter().find(|x| u64::from(x.id.0) == id).and_then(|x| x.text.clone()).unwrap();
    let run = &text.paragraphs[0].runs.last().unwrap();
    assert_eq!(run.text, "[[AD_HOC:my_name]]");
    assert!(matches!(run.kind, deckcraft_model::text::RunKind::Token { .. }), "{run:?}");

    // The pane now lists it, and the name field was cleared for the next one.
    assert_eq!(h.query_all_by_label("my_name").count(), 2, "once in the text-variable list, once under Variables in slides");
    assert_eq!(h.get_by_role_and_label(Role::TextInput, "New variable").value().as_deref(), Some(""));
}

#[test]
fn insert_is_disabled_without_a_text_insertion_point() {
    let s = Session::with_new();
    let mut app = SlideApp::new(s, Services::default());
    app.ui.pane = Some("variables".into());
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut SlideApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    assert!(h.query_by_label_contains("Click into a text box").is_some(), "no text insertion point yet");
    // Clicking does nothing: the name field is disabled along with the button, so there's
    // nothing to insert.
    h.get_by_label("Insert Variable").click();
    h.run_steps(3);
    assert!(h.query_by_label("No variables in this document yet.").is_some(), "still nothing inserted");
}

fn harness_on_the_seismic_tab() -> Harness<'static, SlideApp> {
    let s = Session::with_new();
    let mut app = SlideApp::new(s, Services::default());
    app.ui.tab = "Seismic".into();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut SlideApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h
}

#[test]
fn the_seismic_tab_s_show_variable_panel_button_opens_the_pane() {
    let mut h = harness_on_the_seismic_tab();
    assert_eq!(h.state().ui.pane, None);
    h.get_by_label("Show Variable Panel").click();
    h.run_steps(3);
    assert_eq!(h.state().ui.pane.as_deref(), Some("variables"));
}

#[test]
fn the_seismic_tab_s_save_as_livedoc_pptx_button_downloads_bytes_without_a_file_picker() {
    // `Services::default()` has no `pick_save`, so `export_as` falls back to its web-style
    // "download" path — easiest way to observe that a click actually produced real .pptx bytes
    // without needing a real file picker in a test.
    let s = Session::with_new();
    let downloaded = std::rc::Rc::new(std::cell::RefCell::new(None));
    let d = downloaded.clone();
    let services = Services {
        download: Some(Box::new(move |name: &str, bytes: &[u8]| *d.borrow_mut() = Some((name.to_string(), bytes.to_vec())))),
        ..Default::default()
    };
    let mut app = SlideApp::new(s, services);
    app.ui.tab = "Seismic".into();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut SlideApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h.get_by_label("Save as Livedoc PPTX").click();
    h.run_steps(3);
    let (name, bytes) = downloaded.borrow().clone().expect("the click should have produced a download");
    assert!(name.ends_with(".pptx"), "{name}");
    assert_eq!(&bytes[..2], b"PK", "should be a real zip (.pptx is a zip)");
}

/// Without `SEISMIC_*` credentials configured (the default on a dev machine or CI), clicking
/// Upload fails at once with a clear setup message — no thread, no network call.
#[test]
fn clicking_upload_livedoc_without_credentials_shows_a_setup_message() {
    let mut h = harness_on_the_seismic_tab();
    assert!(h.state().toast.is_none());
    h.get_by_label("Upload Livedoc").click();
    h.run_steps(3);
    let toast = h.state().toast.as_ref().expect("a setup message should show");
    assert!(toast.text.contains("isn't configured"), "{}", toast.text);
}

#[test]
fn a_dynamic_table_is_shown_as_a_table_with_its_columns() {
    let mut s = Session::with_new();
    s.execute("livedoc.insertDynamicTable", &json!({"name": "zzzz", "columns": ["aaa", "bbb"]})).unwrap();
    let mut app = SlideApp::new(s, Services::default());
    app.ui.pane = Some("variables".into());
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut SlideApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    assert!(h.query_by_label("Dynamic tables").is_some());
    assert!(h.query_by_label("aaa").is_some() && h.query_by_label("bbb").is_some(), "the columns head the table");
    assert!(h.query_by_label("zzzz.aaa").is_none(), "a table's columns aren't listed as separate text variables");
    assert!(h.query_by_label("Insert Dynamic Table").is_some());
    assert!(h.query_by_label("Bind Picture").is_some());
}

#[test]
fn the_pane_lists_where_a_variable_is_used_and_all_slides_fills_the_rest() {
    let mut s = Session::with_new();
    s.execute("slide.new", &json!({})).unwrap();
    s.execute("slide.go", &json!({"index": 0})).unwrap();
    s.execute("insert.textBox", &json!({"rect": [100, 100, 300, 40]})).unwrap();
    s.execute("text.insertVariable", &json!({"name": "client"})).unwrap();
    s.execute("text.exit", &json!({})).unwrap();
    let mut app = SlideApp::new(s, Services::default());
    app.ui.pane = Some("variables".into());
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut SlideApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    assert!(h.query_by_label("Variables in slides").is_some());
    let used = |h: &Harness<'_, SlideApp>| deckcraft_livedoc::usage(&h.state().session.active().unwrap().doc).into_iter().next().map(|u| u.slides);
    assert_eq!(used(&h), Some(vec![0]));

    h.get_by_label("All slides").click();
    h.run_steps(3);
    assert_eq!(used(&h), Some(vec![0, 1]), "the second slide got the variable");
}
