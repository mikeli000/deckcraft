//! A shape's `custom_xml_ref` (`p:custDataLst` → `p:custData` → a relationship to an existing
//! document-level part) survives a save and reopen — the generic mechanism `deckcraft-livedoc`
//! uses to bind a shape to its own `Strauss.PersonalizationDefinition` custom-XML item.

use std::io::{Cursor, Read};

use deckcraft_model::{KeptPart, Shape, Slide, defaults};

const CUSTOM_XML_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXml";

#[test]
fn a_shapes_custom_xml_ref_survives_round_trip() {
    let mut p = defaults::new_presentation(None);
    p.kept_parts.push(KeptPart {
        name: "customXml/item1.xml".to_string(),
        xml: "<Existing/>".to_string(),
        rel_type: CUSTOM_XML_REL.to_string(),
        from_root: false,
        ..Default::default()
    });
    let shape = Shape { custom_xml_ref: Some("customXml/item1.xml".to_string()), ..Default::default() };
    p.slides = vec![std::sync::Arc::new(Slide { shapes: vec![shape], ..Default::default() })];

    let bytes = deckcraft_pptx::export(&p).expect("export");
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes.clone())).expect("zip");
    let mut rels = String::new();
    zip.by_name("ppt/slides/_rels/slide1.xml.rels").expect("slide1 rels").read_to_string(&mut rels).expect("read");
    assert!(rels.contains(CUSTOM_XML_REL), "{rels}");
    assert!(rels.contains("item1.xml"), "{rels}");
    let mut slide_xml = String::new();
    zip.by_name("ppt/slides/slide1.xml").expect("slide1").read_to_string(&mut slide_xml).expect("read");
    assert!(slide_xml.contains("<p:custData"), "{slide_xml}");

    let back = deckcraft_pptx::import(&bytes).expect("import");
    assert_eq!(back.slides[0].shapes[0].custom_xml_ref.as_deref(), Some("customXml/item1.xml"));
}

/// `ppt/presentation.xml`'s own `p:custDataLst` lists every document-level customXml `KeptPart`
/// — not just its relationship, the body itself — except one a shape already binds to via its own
/// `custom_xml_ref` (a real LiveDocs-authored sample does the same: a shape-bound item isn't
/// repeated in the presentation's own list). Missing this body entry, even with the relationship
/// present, is why a real tenant's FormService can't find a template's identity metadata.
#[test]
fn presentations_own_cust_data_lst_lists_document_level_kept_parts_but_not_shape_bound_ones() {
    let mut p = defaults::new_presentation(None);
    p.kept_parts.push(KeptPart {
        name: "customXml/item1.xml".to_string(),
        xml: "<ShapeBound/>".to_string(),
        rel_type: CUSTOM_XML_REL.to_string(),
        from_root: false,
        ..Default::default()
    });
    p.kept_parts.push(KeptPart {
        name: "customXml/item2.xml".to_string(),
        xml: "<DocumentLevel/>".to_string(),
        rel_type: CUSTOM_XML_REL.to_string(),
        from_root: false,
        ..Default::default()
    });
    let shape = Shape { custom_xml_ref: Some("customXml/item1.xml".to_string()), ..Default::default() };
    p.slides = vec![std::sync::Arc::new(Slide { shapes: vec![shape], ..Default::default() })];

    let bytes = deckcraft_pptx::export(&p).expect("export");
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    let mut pres_rels = String::new();
    zip.by_name("ppt/_rels/presentation.xml.rels").expect("presentation rels").read_to_string(&mut pres_rels).expect("read");
    let rid_for = |target: &str| -> String {
        let marker = format!("Target=\"{target}\"");
        let start = pres_rels.find(&marker).unwrap_or_else(|| panic!("no relationship to {target} in {pres_rels}"));
        let before = &pres_rels[..start];
        let id_attr = before.rfind("Id=\"").expect("a preceding Id attribute");
        let rest = &before[id_attr + 4..];
        rest[..rest.find('"').unwrap()].to_string()
    };
    let shape_bound_rid = rid_for("../customXml/item1.xml");
    let doc_level_rid = rid_for("../customXml/item2.xml");

    let mut pres_xml = String::new();
    zip.by_name("ppt/presentation.xml").expect("presentation").read_to_string(&mut pres_xml).expect("read");
    assert!(pres_xml.contains(&format!(r#"<p:custData r:id="{doc_level_rid}""#)), "{pres_xml}");
    assert!(
        !pres_xml.contains(&format!(r#"<p:custData r:id="{shape_bound_rid}""#)),
        "the shape-bound item shouldn't also be listed at the presentation level: {pres_xml}"
    );
}
