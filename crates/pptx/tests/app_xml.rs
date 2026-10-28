//! `docProps/app.xml` carries `AppVersion` — some strict external consumers reject a file
//! missing it outright (confirmed against Seismic's FormService, see
//! `deckcraft_livedoc::export::export_pptx_for_live_upload`'s own doc comment), regardless of
//! the document's own content.

use std::io::{Cursor, Read};

#[test]
fn app_xml_carries_an_app_version() {
    let p = deckcraft_model::defaults::new_presentation(None);
    let bytes = deckcraft_pptx::export(&p).expect("export");
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    let mut app = String::new();
    zip.by_name("docProps/app.xml").expect("app.xml").read_to_string(&mut app).expect("read");
    assert!(app.contains("<AppVersion>16.0000</AppVersion>"), "{app}");
}
