//! Exporting a presentation so a real Seismic LiveDocs session recognizes its `AD_HOC` variables:
//! both [`export_pptx_with_livedoc`] and [`export_pptx_for_live_upload`] write the full structure
//! a real LiveDocs-authored file carries — document identity (`Presentation`/`TemplateEditing`),
//! the `AD_HOC`/`Computed`/`System` variable lists (`VariableListDefinition`/`VariableList`/
//! `DataSourceInfo`/`DataSourceMapping`, plus `SourceDataModel` for `AD_HOC`), the
//! `VariableListCustXmlRels` index, `AllExternalAdhocVariableMappings`, and (since a real sample
//! confirmed this is part of the baseline, not an upload-only extra) a `Strauss.
//! PersonalizationDefinition` item per shape holding a variable — unconditionally, even with zero
//! variables, matching a real, independently-verified sample (`000_Presentation2_v0.1`) exactly.
//! The only difference between the two is `tenant`: [`export_pptx_for_live_upload`] takes the
//! real one (for `TemplateEditing`'s `TenantOrigin`); [`export_pptx_with_livedoc`] has none yet
//! (a plain local save, before any tenant is involved) and writes an empty one.
//!
//! **Partially verified against a real tenant, still not fully working**: uploading a document
//! exported through [`export_pptx_for_live_upload`] to a real tenant
//! (`upload::tests::real_upload_live_pptx_against_a_real_tenant`, `#[ignore]`d) succeeds through
//! the first three of `upload::upload_live_pptx`'s four calls (token, blob, create content — the
//! part that actually matters, a real content item really gets created). The fourth, best-effort
//! call ("attach a default form") still fails with `"Missing metadata xml in template
//! source:Presentation"`, the exact same error a document with *no* skeleton at all produces —
//! neither this skeleton, fixing `deckcraft-pptx`'s `docProps/app.xml` to carry `AppVersion`, nor
//! the per-shape `Strauss.PersonalizationDefinition` binding below (all three separately confirmed
//! real gaps) have resolved it yet. The real cause is still unknown; verifying it further needs
//! either a real LiveDocs-authored sample with a variable to diff against (none was available when
//! this doc comment was first written — see [`crate::recognize_variables`]'s own doc comment) or
//! someone with lower-level FormService knowledge, not more blind guessing against a real tenant.
//!
//! **The per-shape binding** (`<p:custData>` on the shape in `slideN.xml`, pointing at a
//! `Strauss.PersonalizationDefinition` custom-XML item) is implemented in
//! [`export_pptx_for_live_upload`] (`attach_personalization`), via `deckcraft-pptx`'s generic
//! `Shape::custom_xml_ref` hook — confirmed structurally against a real, independently-verified
//! LiveDocs-authored sample (`000_Presentation2_v0.1`)'s own `customXml/item22.xml` and
//! `slide1.xml`/`slide1.xml.rels`. Not yet re-verified against a real tenant (see above).

use deckcraft_model::{KeptPart, Presentation, ShapeKind};

/// Exports `presentation` with the full LiveDocs structure (see this module's own doc comment),
/// `tenant` unknown (a plain local save happens before any tenant is involved — see
/// [`export_pptx_for_live_upload`] for the real-tenant counterpart `upload::upload_live_pptx`
/// needs).
pub fn export_pptx_with_livedoc(presentation: &Presentation) -> Result<Vec<u8>, deckcraft_pptx::PptxError> {
    export_pptx_for_live_upload(presentation, "")
}

/// Exports `presentation` with the full LiveDocs structure (see this module's own doc comment),
/// `tenant` as the real target tenant — what `upload::upload_live_pptx`'s own `pptx_bytes`
/// argument needs.
pub fn export_pptx_for_live_upload(presentation: &Presentation, tenant: &str) -> Result<Vec<u8>, deckcraft_pptx::PptxError> {
    let (names, tables) = ad_hoc_bindings(presentation);
    let mut augmented = presentation.clone();
    let mut skeleton = upload_skeleton_parts(&names, &tables, tenant, presentation.kept_parts.len());
    augmented.kept_parts.extend(skeleton.parts);
    attach_personalization(&mut augmented, &skeleton.ad_hoc, &mut skeleton.next_item, &skeleton.salt);
    deckcraft_pptx::export(&augmented)
}

/// One dynamic-table variable the `AD_HOC` list needs: `name` is the table's own variable name
/// (`zzzz` in `[[AD_HOC:zzzz.aaa]]`), `columns` is each column's own leaf name in left-to-right
/// order (`aaa`, `bbb`, `ccc`) — see [`crate::DynamicTableBinding`]'s own doc comment for where
/// this comes from.
struct TableSpec {
    name: String,
    columns: Vec<String>,
}

/// Every scalar variable name and dynamic-table binding the `AD_HOC` list needs — everything
/// [`crate::list_variables`] would report, except split so a table's own column tokens (`zzzz.aaa`)
/// don't *also* become flat scalar variables; they're nested under their own table entry instead.
/// A picture's image variable needs no `AD_HOC` field at all (see [`attach_personalization`]'s own
/// doc comment for why) and so isn't collected here.
fn ad_hoc_bindings(presentation: &Presentation) -> (Vec<String>, Vec<TableSpec>) {
    let mut names = Vec::new();
    let mut tables = Vec::new();
    for slide in &presentation.slides {
        deckcraft_model::walk(&slide.shapes, &mut |shape, _depth| {
            if let Some(body) = &shape.text {
                for paragraph in &body.paragraphs {
                    for run in &paragraph.runs {
                        if let Some(name) = crate::variable_token_name(run)
                            && !names.contains(&name)
                        {
                            names.push(name);
                        }
                    }
                }
            }
            if let ShapeKind::Table(table) = &shape.kind
                && let Some(binding) = crate::dynamic_table_binding(table)
            {
                tables.push(TableSpec { name: binding.table_name, columns: binding.columns.into_iter().map(|(name, _)| name).collect() });
            }
        });
    }
    (names, tables)
}

/// Binds each shape holding a recognized variable (a plain text token, a recognized dynamic-table
/// row, or a tagged image variable) to its `Strauss.PersonalizationDefinition` custom-XML item
/// (the same "item/itemProps/rels" triple every other part this module writes gets) via that
/// shape's `custom_xml_ref` (see `deckcraft_pptx::write::shapes::shape_cust_data`) — confirmed
/// structurally against a real, independently-verified LiveDocs-authored sample's own
/// `customXml/item22.xml` plus its `slide1.xml`'s
/// `<p:custDataLst><p:custData r:id="..."/></p:custDataLst>` and `slide1.xml.rels`' matching
/// relationship. A plain text shape only ever binds its first variable — a real sample's own text
/// `PersonalizationDefinition` only ever had one `DataReference`, and this crate doesn't model more
/// than one text binding per shape.
///
/// An image variable needs no `AD_HOC` field at all: a real sample's own `pd:ExternalContentPD`
/// item (`customXml/item19.xml`/`item39.xml` in `(New) Test_v1.0.PPTX`) carries an *empty*
/// `pd:DataReferenceList` — the binding is entirely self-contained (a `uid`, and a free-text
/// `name`), nothing an `AD_HOC` variable list needs to know about.
fn attach_personalization(p: &mut Presentation, ad_hoc: &ListRelEntry, n: &mut usize, salt: &str) {
    let mut scalar_vars: Vec<(deckcraft_model::ShapeId, String)> = Vec::new();
    let mut table_bindings: Vec<(deckcraft_model::ShapeId, crate::DynamicTableBinding)> = Vec::new();
    let mut image_vars: Vec<(deckcraft_model::ShapeId, String)> = Vec::new();
    for slide in &p.slides {
        deckcraft_model::walk(&slide.shapes, &mut |shape, _depth| {
            if let Some(body) = &shape.text
                && let Some(name) = body.paragraphs.iter().flat_map(|pa| pa.runs.iter()).find_map(crate::variable_token_name)
            {
                scalar_vars.push((shape.id, name));
            }
            match &shape.kind {
                ShapeKind::Table(table) => {
                    if let Some(binding) = crate::dynamic_table_binding(table) {
                        table_bindings.push((shape.id, binding));
                    }
                }
                ShapeKind::Picture { .. } => {
                    if let Some(name) = crate::image_variable_name(shape) {
                        image_vars.push((shape.id, name));
                    }
                }
                _ => {}
            }
        });
    }

    let mut item_names = std::collections::HashMap::new();
    let new_item = |p: &mut Presentation, n: &mut usize, xml: &str, item_id_seed: String| -> String {
        let item_n = next_item(n);
        let item_id = fake_guid(&item_id_seed);
        p.kept_parts.push(item_part(item_n, xml));
        p.kept_parts.push(item_props_part(item_n, &item_id));
        p.kept_parts.push(item_rels_part(item_n));
        format!("customXml/item{item_n}.xml")
    };

    for (shape_id, var_name) in &scalar_vars {
        let Some(field_id) = ad_hoc.field_ids.iter().find(|(name, _)| name == var_name).map(|(_, id)| id.clone()) else { continue };
        let xml = personalization_definition_xml(&ad_hoc.data_source_id, &field_id, &ad_hoc.list_id);
        let part_name = new_item(p, n, &xml, format!("{salt}:personalization:{}", shape_id.0));
        item_names.insert(*shape_id, part_name);
    }

    for (shape_id, binding) in &table_bindings {
        let field_id = |name: &str| ad_hoc.field_ids.iter().find(|(n, _)| n == name).map(|(_, id)| id.clone());
        let Some(table_field_id) = field_id(&binding.table_name) else { continue };
        let column_field_ids: Option<Vec<String>> =
            binding.columns.iter().map(|(col, _)| field_id(&format!("{}.{col}", binding.table_name))).collect();
        let Some(column_field_ids) = column_field_ids else { continue };
        let xml = dynamic_table_descriptor_xml(
            &ad_hoc.data_source_id,
            &ad_hoc.list_id,
            &binding.table_name,
            &table_field_id,
            &column_field_ids,
            binding.row,
            salt,
        );
        let part_name = new_item(p, n, &xml, format!("{salt}:personalization:table:{}", shape_id.0));
        item_names.insert(*shape_id, part_name);
    }

    for (shape_id, var_name) in &image_vars {
        let uid = fake_guid_plain(&format!("{salt}:personalization:image:{}:uid", shape_id.0));
        let xml = external_content_pd_xml(var_name, &uid);
        let part_name = new_item(p, n, &xml, format!("{salt}:personalization:image:{}", shape_id.0));
        item_names.insert(*shape_id, part_name);
    }

    if item_names.is_empty() {
        return;
    }
    for slide in &mut p.slides {
        let slide = std::sync::Arc::make_mut(slide);
        crate::walk_mut(&mut slide.shapes, &mut |shape| {
            if let Some(name) = item_names.get(&shape.id) {
                shape.custom_xml_ref = Some(name.clone());
            }
        });
    }
}

/// `Strauss.PersonalizationDefinition` XML for one shape bound to one `AD_HOC` field — matches a
/// real sample's `customXml/item22.xml` exactly (its `name`/`desc`/`uid` attributes were all
/// empty there too).
fn personalization_definition_xml(data_source_id: &str, field_id: &str, list_id: &str) -> String {
    format!(
        concat!(
            r#"<pd:PersonalizationDefinition name="" xmlns:pd="Strauss.PersonalizationDefinition">"#,
            "<pd:DataReferenceList>{data_ref}</pd:DataReferenceList>",
            r#"<pd:VariableReplacementDescriptor name="" desc="" uid="">"#,
            "<pd:DataReferenceList>{data_ref}</pd:DataReferenceList>",
            "</pd:VariableReplacementDescriptor></pd:PersonalizationDefinition>"
        ),
        data_ref = format!(r#"<pd:DataReference datasourceID="{data_source_id}" dataFieldID="{field_id}" variableListUniqueId="{list_id}"/>"#)
    )
}

/// `Strauss.PersonalizationDefinition` XML for a table shape bound to a dynamic-table variable —
/// matches a real sample's own `customXml/item38.xml` (`(New) Test_v1.0.PPTX`) structurally:
/// the table's own field plus every column field in the outer `pd:DataReferenceList`, a
/// `pd:DynamicTableDescriptor` naming the table variable, one `pd:BindTable` repeating
/// `repeating_row` (0-indexed; written 1-indexed, matching the real sample's own convention), and
/// a `pd:DynamicTableColumn` per cell — empty for every row before `repeating_row` (the header),
/// one `pd:DataReference` each for `repeating_row` itself.
fn dynamic_table_descriptor_xml(
    data_source_id: &str,
    list_id: &str,
    table_name: &str,
    table_field_id: &str,
    column_field_ids: &[String],
    repeating_row: usize,
    salt: &str,
) -> String {
    let data_ref =
        |field_id: &str| format!(r#"<pd:DataReference datasourceID="{data_source_id}" dataFieldID="{field_id}" variableListUniqueId="{list_id}"/>"#);
    let all_refs: String = column_field_ids.iter().map(|id| data_ref(id)).collect();
    let uid = fake_guid_plain(&format!("{salt}:dynamicTable:{table_name}"));

    let header_columns: String = (0..repeating_row)
        .flat_map(|ri| (0..column_field_ids.len()).map(move |ci| (ri, ci)))
        .map(|(ri, ci)| format!(r#"<pd:DynamicTableColumn rowIndex="{ri}" colIndex="{ci}"><pd:DataReferenceList/></pd:DynamicTableColumn>"#))
        .collect();
    let data_columns: String = column_field_ids
        .iter()
        .enumerate()
        .map(|(ci, id)| {
            format!(
                r#"<pd:DynamicTableColumn rowIndex="{repeating_row}" colIndex="{ci}"><pd:DataReferenceList>{}</pd:DataReferenceList></pd:DynamicTableColumn>"#,
                data_ref(id)
            )
        })
        .collect();

    format!(
        concat!(
            r#"<pd:PersonalizationDefinition name="" xmlns:pd="Strauss.PersonalizationDefinition">"#,
            "<pd:DataReferenceList>{all_refs}</pd:DataReferenceList>",
            r#"<pd:DynamicTableDescriptor name="{table_name}" desc="" uid="{uid}" isHorizontal="False" overflowToTableId="" continueFromTableId="" hideSlideWhenNoData="False" fixWidth="False" AdvancedWeighting="False" maxWeight="100">"#,
            "<pd:DataReferenceList>{all_refs}</pd:DataReferenceList>",
            r#"<pd:BindTables><pd:BindTable IsLimitRecordCount="False" RepeatingRowIndexFrom="{row1}" RepeatingRowIndexTo="{row1}" MaxRecords="999" isFooterInLastPage="True" isHeaderInFirstPage="False" startDataIndex="0" maxRowPerSlide="10" IsMaxLinesPerSlide="False" HeaderRowIndexFrom="0" HeaderRowIndexTo="{header_to}" FooterRowIndexFrom="0" FooterRowIndexTo="0" AnchorToBottom="False" IsFromOriginalTable="False" FooterAnchorToBottom="False">"#,
            "{table_ref}",
            r#"<pd:SortDescriptor name="" desc="" uid=""/><pd:Condition rel="&amp;&amp;"/><pd:DecimalAlign name="" desc="" uid="" AlignDecimal="False"/>"#,
            "</pd:BindTable></pd:BindTables>",
            "<pd:DynamicTableColumns>{header_columns}{data_columns}</pd:DynamicTableColumns>",
            "</pd:DynamicTableDescriptor></pd:PersonalizationDefinition>"
        ),
        all_refs = all_refs,
        table_name = xml_escape(table_name),
        uid = uid,
        row1 = repeating_row + 1,
        header_to = repeating_row.saturating_sub(1),
        table_ref = data_ref(table_field_id),
        header_columns = header_columns,
        data_columns = data_columns,
    )
}

/// `Strauss.PersonalizationDefinition` XML for a picture bound to an image variable — matches a
/// real sample's own `customXml/item39.xml` (`(New) Test_v1.0.PPTX`) exactly: an *empty*
/// `pd:DataReferenceList` (no `AD_HOC` field involved at all, see [`attach_personalization`]'s own
/// doc comment) and a `pd:ExternalContentPD` carrying `name` as a free-text label.
fn external_content_pd_xml(name: &str, uid: &str) -> String {
    format!(
        concat!(
            r#"<pd:PersonalizationDefinition name="" xmlns:pd="Strauss.PersonalizationDefinition">"#,
            "<pd:DataReferenceList/>",
            r#"<pd:ExternalContentPD name="{name}" desc="" uid="{uid}" provider="" control="ScaleToFit" hasDefault="False" canManualUpload="True" allowPartialPdf="True" OverlayForPage="0" IsFitPDF="True" IsAdvancedMode="False" SamplePDFName="" ExternalContentType="ExternalImage" AllowPDF="False" AllowPptx="True" IncludeStandardPPTX="True" IncludeLiveDoc="True" canSelectFromLibrary="True" isFromExternal="False" SlideFitPattern="AlignTopLeft" IncludePlaceholder="False">"#,
            "<pd:DataReferenceList/>",
            r#"<pd:ExternalContentQuery queryType="PullFromLibrary" queryFolderId="" rel="&amp;&amp;"/>"#,
            "</pd:ExternalContentPD></pd:PersonalizationDefinition>"
        ),
        name = xml_escape(name),
        uid = uid,
    )
}

/// One variable, with the ids it's referenced by inside the parts below.
struct VarInfo {
    name: String,
    field_id: String,
}

/// One `VariableListCustXmlRels` entry: the `ds:itemID`s of a list's own parts. `sdmc` is `Some`
/// only for lists that get a `SourceDataModel` part (`AD_HOC`, not `Computed`/`System` — matches
/// the asymmetry a real sample's `VariableListCustXmlRels` has).
#[derive(Clone)]
struct ListRelEntry {
    name: String,
    def: String,
    lib: String,
    dsi: String,
    dsm: String,
    sdmc: Option<String>,
    /// This list's own content-level ids, for [`attach_personalization`]'s `AD_HOC`-only use —
    /// unused (and always present) for `Computed`/`System`.
    list_id: String,
    data_source_id: String,
    /// `(variable name, dataFieldId)`, empty for `Computed`/`System` (no variables).
    field_ids: Vec<(String, String)>,
}

/// Running counter for `customXml/itemN.xml` (and its same-numbered `itemPropsN.xml`/
/// `_rels/itemN.xml.rels` sidecar — matching a real sample's own convention, where every
/// `itemN.xml` pairs with `itemPropsN.xml` of the exact same `N`, never a different one), numbered
/// contiguously past whatever the document already kept from import so new parts never collide
/// with an existing one.
fn next_item(n: &mut usize) -> usize {
    *n += 1;
    *n
}

/// One variable list's full part set — `VariableListDefinition`/`VariableList`/`DataSourceInfo`/
/// `DataSourceMapping`, plus `SourceDataModel` when `include_sdmc` (only `AD_HOC` ever has real
/// data to source from) — each its own `customXml/itemN.xml` + `itemPropsN.xml` +
/// `_rels/itemN.xml.rels` triple. `names`/`tables` are empty for `Computed`/`System` (this editor's
/// plugin never creates either kind), in which case these same functions already produce an empty
/// `<Variable>`/`<FieldInfo>`/`<RawMapping>` list — no separate "empty" variant needed.
fn one_list_parts(
    name: &str,
    mapping_name: &str,
    data_source_type: &str,
    names: &[String],
    tables: &[TableSpec],
    include_sdmc: bool,
    salt: &str,
    n: &mut usize,
) -> (Vec<KeptPart>, ListRelEntry) {
    let list_id = fake_guid_plain(&format!("{salt}:{name}:list"));
    let data_source_id = fake_guid_plain(&format!("{salt}:{name}:dataSource"));
    let scalars: Vec<VarInfo> =
        names.iter().map(|v| VarInfo { name: v.clone(), field_id: fake_guid_plain(&format!("{salt}:{name}:field:{v}")) }).collect();
    let table_entries: Vec<(VarInfo, Vec<VarInfo>)> = tables
        .iter()
        .map(|t| {
            let table_field = VarInfo { name: t.name.clone(), field_id: fake_guid_plain(&format!("{salt}:{name}:table:{}", t.name)) };
            let cols = t
                .columns
                .iter()
                .map(|c| VarInfo { name: c.clone(), field_id: fake_guid_plain(&format!("{salt}:{name}:table:{}:{c}", t.name)) })
                .collect();
            (table_field, cols)
        })
        .collect();

    let mut xmls = vec![
        variable_list_definition_xml(name, &list_id, &data_source_id, &scalars, &table_entries),
        variable_list_xml(name, &list_id, &data_source_id),
        data_source_info_xml(name, data_source_type, &data_source_id, &scalars, &table_entries),
        data_source_mapping_xml(mapping_name, &data_source_id, &scalars, &table_entries),
    ];
    if include_sdmc {
        xmls.push(source_data_model_xml(name, &data_source_id));
    }

    let mut out = Vec::with_capacity(xmls.len() * 3);
    let mut item_ids = Vec::with_capacity(xmls.len());
    for xml in &xmls {
        let item_n = next_item(n);
        let item_id = fake_guid(&format!("{salt}:{name}:itemProps:{item_n}"));
        out.push(item_part(item_n, xml));
        out.push(item_props_part(item_n, &item_id));
        out.push(item_rels_part(item_n));
        item_ids.push(item_id);
    }
    let mut field_ids: Vec<(String, String)> = scalars.iter().map(|v| (v.name.clone(), v.field_id.clone())).collect();
    for (table_field, cols) in &table_entries {
        field_ids.push((table_field.name.clone(), table_field.field_id.clone()));
        for c in cols {
            field_ids.push((format!("{}.{}", table_field.name, c.name), c.field_id.clone()));
        }
    }
    let rel = ListRelEntry {
        name: name.to_string(),
        def: item_ids[0].clone(),
        lib: item_ids[1].clone(),
        dsi: item_ids[2].clone(),
        dsm: item_ids[3].clone(),
        sdmc: include_sdmc.then(|| item_ids[4].clone()),
        list_id,
        data_source_id,
        field_ids,
    };
    (out, rel)
}

/// Same "item/itemProps/rels" triple as every other part this module writes — a real sample's own
/// `VariableListCustXmlRels` item (`item20.xml` there) gets one too, not a bare item with no
/// sidecar.
fn rels_index_part(entries: &[ListRelEntry], salt: &str, n: &mut usize) -> [KeptPart; 3] {
    let item_n = next_item(n);
    let item_id = fake_guid(&format!("{salt}:variableListCustXmlRels:itemProps"));
    [item_part(item_n, &variable_list_cust_xml_rels_xml(entries)), item_props_part(item_n, &item_id), item_rels_part(item_n)]
}

/// [`upload_skeleton_parts`]'s result: the parts themselves, plus what [`attach_personalization`]
/// needs to bind individual shapes to the `AD_HOC` list afterwards (`next_item`, so its own new
/// items number past these; `salt`, so its ids are mixed from the same per-export randomness).
struct Skeleton {
    parts: Vec<KeptPart>,
    next_item: usize,
    salt: String,
    ad_hoc: ListRelEntry,
}

/// The baseline "this is a LiveDocs template" skeleton a real LiveDocs-authored file carries
/// unconditionally, plus `AD_HOC`'s own parts (always written now, same as `Computed`/`System` —
/// a real FormService call still failed on a document with zero variables until `AD_HOC` also got
/// an unconditional `VariableListCustXmlRels` entry; see this module's own doc comment).
fn upload_skeleton_parts(names: &[String], tables: &[TableSpec], tenant: &str, start: usize) -> Skeleton {
    let salt = fresh_salt();
    let mut n = start;
    let mut out = Vec::new();

    // Document identity: a fresh UniqueId per export (no way to persist one across exports of
    // the same document), and no CID (a brand-new upload has no library content id yet).
    let root_id = fake_guid_plain(&format!("{salt}:template:rootElementId:{tenant}"));
    out.extend(identity_part_triple(
        &mut n,
        &salt,
        "presentation",
        format!(r#"<Presentation UniqueId="{root_id}" MajorVersion="0" MinorVersion="1"/>"#),
    ));
    out.extend(identity_part_triple(
        &mut n,
        &salt,
        "templateEditing",
        format!("<TemplateEditing><RootElementId>{root_id}</RootElementId><TenantOrigin>{}</TenantOrigin></TemplateEditing>", xml_escape(tenant)),
    ));

    let mut rels = Vec::with_capacity(3);
    for (name, data_source_type) in [("Computed", "Expression"), ("System", "System")] {
        let (parts, rel) = one_list_parts(name, "EXPRESSION_VARIABLE_MAPPING", data_source_type, &[], &[], false, &salt, &mut n);
        out.extend(parts);
        rels.push(rel);
    }
    let (parts, rel) = one_list_parts("AD_HOC", "AD_HOC_MAPPING", "Ad_Hoc", names, tables, true, &salt, &mut n);
    out.extend(parts);
    let ad_hoc = rel.clone();
    rels.push(rel);

    out.extend(identity_part_triple(&mut n, &salt, "allExternalAdhocVariableMappings", "<AllExternalAdhocVariableMappings/>".to_string()));
    out.extend(rels_index_part(&rels, &salt, &mut n));
    Skeleton { parts: out, next_item: n, salt, ad_hoc }
}

/// A document-identity-style part (document identity, `AllExternalAdhocVariableMappings`) with
/// no `ds:itemID` pinned anywhere else — still gets its own `itemPropsN.xml`/`_rels` sidecar,
/// same OPC shape every other customXml item in this module gets, just one nothing else points
/// back to.
fn identity_part_triple(n: &mut usize, salt: &str, seed: &str, xml: String) -> [KeptPart; 3] {
    let item_n = next_item(n);
    let item_id = fake_guid(&format!("{salt}:{seed}:itemProps"));
    [item_part(item_n, &xml), item_props_part(item_n, &item_id), item_rels_part(item_n)]
}

const CUSTOM_XML_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXml";
const CUSTOM_XML_PROPS_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXmlProps";
const CUSTOM_XML_PROPS_CT: &str = "application/vnd.openxmlformats-officedocument.customXmlProperties+xml";

fn item_part(n: usize, xml: &str) -> KeptPart {
    KeptPart {
        name: format!("customXml/item{n}.xml"),
        xml: xml.to_string(),
        rel_type: CUSTOM_XML_REL.to_string(),
        from_root: false,
        ..Default::default()
    }
}

fn item_props_part(n: usize, item_id: &str) -> KeptPart {
    KeptPart {
        name: format!("customXml/itemProps{n}.xml"),
        content_type: CUSTOM_XML_PROPS_CT.to_string(),
        xml: format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><ds:datastoreItem ds:itemID="{item_id}" xmlns:ds="http://schemas.openxmlformats.org/officeDocument/2006/customXml"><ds:schemaRefs/></ds:datastoreItem>"#
        ),
        ..Default::default()
    }
}

/// Matches a real sample's own convention exactly: `itemN.xml.rels` always points at
/// `itemPropsN.xml` of the *same* number, never a different one.
fn item_rels_part(n: usize) -> KeptPart {
    KeptPart {
        name: format!("customXml/_rels/item{n}.xml.rels"),
        xml: format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{CUSTOM_XML_PROPS_REL}" Target="itemProps{n}.xml"/></Relationships>"#
        ),
        ..Default::default()
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// `<Variable name="..." type="STRING" dataFieldId="..."/>`, one per scalar.
fn scalar_variable_xml(v: &VarInfo) -> String {
    format!(r#"<Variable name="{}" type="STRING" dataFieldId="{}"/>"#, xml_escape(&v.name), v.field_id)
}

fn variable_list_definition_xml(name: &str, list_id: &str, data_source_id: &str, scalars: &[VarInfo], tables: &[(VarInfo, Vec<VarInfo>)]) -> String {
    let mut variables: String = scalars.iter().map(scalar_variable_xml).collect();
    for (table_field, cols) in tables {
        let children: String = cols.iter().map(scalar_variable_xml).collect();
        variables += &format!(
            r#"<Variable name="{}" type="TABLE" dataFieldId="{}" isRepeatingTable="True">{children}</Variable>"#,
            xml_escape(&table_field.name),
            table_field.field_id,
        );
    }
    format!(
        r#"<VariableListDefinition name="{name}" displayName="{name}" id="{list_id}" isdomainofvalue="False" dataSourceId="{data_source_id}">{variables}</VariableListDefinition>"#
    )
}

fn variable_list_xml(name: &str, list_id: &str, data_source_id: &str) -> String {
    format!(
        r#"<VariableList UniqueId="{list_id}" Name="{name}" ContentType="XML" MajorVersion="0" MinorVersion="1" isLocalCopy="False" IsBaseObject="False" DataSourceId="{data_source_id}" DataSourceMajorVersion="0" DataSourceMinorVersion="1"/>"#
    )
}

/// The attribute body every `FieldInfo`/`TableInfo` shares — `original_name` is the scalar's own
/// name (matches a real sample's scalar fields) or empty (matches its table and column fields,
/// whose `OriginalName` is always empty there, unlike their own `Name`).
fn field_info_body_xml(id: &str, name: &str, original_name: &str) -> String {
    format!(
        concat!(
            "<Id>{id}</Id><Name>{name}</Name><Description/><ExpressionString/>",
            "<FieldType>System.String</FieldType><Filter/><DefaultValue/>",
            "<IsInputParameter>false</IsInputParameter><IsInputMultipleValues>false</IsInputMultipleValues>",
            "<IncludeInRecordSelector>true</IncludeInRecordSelector><IsRepeating>false</IsRepeating>",
            "<IsKeyField>false</IsKeyField><Required>false</Required><NativeSourceType>String</NativeSourceType>",
            "<MaxLength>0</MaxLength><DynamicDomainOfValueSourceId/><TrimEmptyRows>false</TrimEmptyRows>",
            "<CanIncludeInModel>true</CanIncludeInModel><IncludeInModel>true</IncludeInModel>",
            "<IsPartial>false</IsPartial><IsValid>true</IsValid><NativeFieldMapping/>",
            "<OriginalName>{original_name}</OriginalName><ValidationMessage/>"
        ),
        id = id,
        name = name,
        original_name = original_name,
    )
}

fn scalar_field_info_xml(v: &VarInfo) -> String {
    let name = xml_escape(&v.name);
    format!("<FieldInfo>{}</FieldInfo>", field_info_body_xml(&v.field_id, &name, &name))
}

/// `<TableInfo>` for a dynamic-table field: same attribute shape as `<FieldInfo>` (an empty
/// `OriginalName`, matching a real sample), wrapping one nested `<FieldInfo>` per column (also an
/// empty `OriginalName` — a real sample's own column fields never repeat their `Name` there).
fn table_info_xml(table_field: &VarInfo, cols: &[VarInfo]) -> String {
    let children: String =
        cols.iter().map(|c| format!("<FieldInfo>{}</FieldInfo>", field_info_body_xml(&c.field_id, &xml_escape(&c.name), ""))).collect();
    format!("<TableInfo>{}{children}</TableInfo>", field_info_body_xml(&table_field.field_id, &xml_escape(&table_field.name), ""))
}

fn data_source_info_xml(name: &str, data_source_type: &str, data_source_id: &str, scalars: &[VarInfo], tables: &[(VarInfo, Vec<VarInfo>)]) -> String {
    let mut fields: String = scalars.iter().map(scalar_field_info_xml).collect();
    for (table_field, cols) in tables {
        fields += &table_info_xml(table_field, cols);
    }
    format!(
        concat!(
            "<DataSourceInfo><Id>{data_source_id}</Id><MajorVersion>0</MajorVersion><MinorVersion>1</MinorVersion>",
            "<DataSourceType>{data_source_type}</DataSourceType><Name>{name}</Name><Description/><Filter/>",
            "<DataFields>{fields}</DataFields></DataSourceInfo>"
        ),
        data_source_id = data_source_id,
        data_source_type = data_source_type,
        name = name,
        fields = fields
    )
}

fn data_source_mapping_xml(mapping_name: &str, data_source_id: &str, scalars: &[VarInfo], tables: &[(VarInfo, Vec<VarInfo>)]) -> String {
    let mapping_id = fake_guid_plain(&format!("{mapping_name}:mapping:{data_source_id}"));
    let mut raw_mappings: String =
        scalars.iter().map(|v| format!(r#"<RawMapping DataFieldId="{}" Mapping="DataRecord/{}"/>"#, v.field_id, xml_escape(&v.name))).collect();
    for (table_field, cols) in tables {
        let table_name = xml_escape(&table_field.name);
        raw_mappings += &format!(r#"<RawMapping DataFieldId="{}" Mapping="DataRecord/{table_name}"/>"#, table_field.field_id);
        for c in cols {
            raw_mappings += &format!(r#"<RawMapping DataFieldId="{}" Mapping="DataRecord/{table_name}/{}"/>"#, c.field_id, xml_escape(&c.name));
        }
    }
    format!(
        concat!(
            "<DataSourceMapping><Id>{mapping_id}</Id><Name>{mapping_name}</Name>",
            "<TargetDataSource>{data_source_id}</TargetDataSource><SourceType>XML File</SourceType>",
            "<IsReadOnly>false</IsReadOnly><SalesforceOrganizationId>00000000-0000-0000-0000-000000000000</SalesforceOrganizationId>",
            "<SalesforceOrganizationName/><SalesforceApiVersion/>",
            r#"<Properties><Property Name="RecordSeperator" Value="SampleData/DataRecord"/></Properties>"#,
            "<RawMappings>{raw_mappings}</RawMappings><DesignTimeProperties/></DataSourceMapping>"
        ),
        mapping_id = mapping_id,
        mapping_name = mapping_name,
        data_source_id = data_source_id,
        raw_mappings = raw_mappings
    )
}

fn source_data_model_xml(name: &str, data_source_id: &str) -> String {
    format!(r#"<SourceDataModel Name="{name}" TargetDataSourceId="{data_source_id}"/>"#)
}

/// The index tying each variable list's name to the `ds:itemID`s of its own parts.
fn variable_list_cust_xml_rels_xml(entries: &[ListRelEntry]) -> String {
    let rels: String = entries
        .iter()
        .map(|e| {
            let sdmc = e.sdmc.as_deref().map(|id| format!("<SdmcCustXmlId>{id}</SdmcCustXmlId>")).unwrap_or_default();
            format!(
                concat!(
                    r#"<VariableListCustXmlRel variableListName="{name}">"#,
                    "<VariableListDefCustXmlId>{def}</VariableListDefCustXmlId>",
                    "<LibraryMetadataCustXmlId>{lib}</LibraryMetadataCustXmlId>",
                    "<DataSourceInfoCustXmlId>{dsi}</DataSourceInfoCustXmlId>",
                    "<DataSourceMappingCustXmlId>{dsm}</DataSourceMappingCustXmlId>",
                    "{sdmc}",
                    "</VariableListCustXmlRel>"
                ),
                name = e.name,
                def = e.def,
                lib = e.lib,
                dsi = e.dsi,
                dsm = e.dsm,
                sdmc = sdmc
            )
        })
        .collect();
    format!("<VariableListCustXmlRels>{rels}</VariableListCustXmlRels>")
}

/// A value that differs across separate calls within the same process (not just across distinct
/// seeds within one call) — mixed into every id above so two different documents exported by the
/// same long-lived process don't end up with identical `dataSourceId`/`dataFieldId` values
/// whenever they share a variable name.
fn fresh_salt() -> String {
    use std::hash::{BuildHasher, Hasher};
    #[allow(unused_mut)] // mutated below on non-wasm targets only; wasm32 has no SystemTime::now.
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    // `SystemTime::now()` panics on wasm32-unknown-unknown ("time not implemented on this
    // platform") — the exact crash that hit deckcraft-web exporting a livedoc document (see
    // crates/engine/src/recovery.rs's instance_id() for the same fix, same reasoning: on wasm,
    // RandomState's own per-process randomness is already enough entropy on its own).
    #[cfg(not(target_arch = "wasm32"))]
    hasher.write_u64(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0));
    format!("{:016x}", hasher.finish())
}

/// A GUID-*shaped* string, deterministic from `seed` — this crate has no cryptographic-randomness
/// requirement, only "looks like a GUID" and "distinct per distinct seed" (callers mix
/// [`fresh_salt`] into `seed` for uniqueness across exports). Braces, uppercase: the convention a
/// real LiveDocs file uses for `ds:itemID`.
fn fake_guid(seed: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h1 = std::collections::hash_map::DefaultHasher::new();
    seed.hash(&mut h1);
    0x00C0_FFEEu32.hash(&mut h1);
    let a = h1.finish();
    let mut h2 = std::collections::hash_map::DefaultHasher::new();
    seed.hash(&mut h2);
    0x00FA_CADEu32.hash(&mut h2);
    let b = h2.finish();
    format!("{{{:08X}-{:04X}-{:04X}-{:04X}-{:012X}}}", (a >> 32) as u32, (a >> 16) as u16, a as u16, (b >> 48) as u16, b & 0xFFFF_FFFF_FFFF)
}

/// Same value as [`fake_guid`], lowercase with no braces — the convention every content-level id
/// in a real LiveDocs file uses (`VariableList UniqueId=`, `DataSourceId=`, `dataFieldId=`…).
fn fake_guid_plain(seed: &str) -> String {
    let g = fake_guid(seed);
    g.trim_start_matches('{').trim_end_matches('}').to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_with_variable(name: &str) -> Presentation {
        let mut p = deckcraft_model::defaults::new_presentation(None);
        let shape =
            deckcraft_model::Shape { text: Some(deckcraft_model::text::TextBody::from_text(&crate::display_text(name))), ..Default::default() };
        p.slides = vec![std::sync::Arc::new(deckcraft_model::Slide { shapes: vec![shape], ..Default::default() })];
        crate::recognize_variables(&mut p);
        p
    }

    /// A 2-row, 3-column table shape — row 0 plain header text, row 1 `[[AD_HOC:{table}.col]]`
    /// tokens — matching a real add-in-authored sample's own dynamic-table convention (see
    /// `crate::dynamic_table_binding`'s own doc comment).
    fn doc_with_dynamic_table(table_name: &str) -> Presentation {
        let mut p = deckcraft_model::defaults::new_presentation(None);
        let cell = |text: &str| deckcraft_model::table::Cell::new(text);
        let table = deckcraft_model::table::Table {
            rows: vec![
                deckcraft_model::table::Row { height: 20.0, cells: vec![cell("aaa"), cell("bbb"), cell("ccc")] },
                deckcraft_model::table::Row {
                    height: 20.0,
                    cells: vec![
                        cell(&format!("[[AD_HOC:{table_name}.aaa]]")),
                        cell(&format!("[[AD_HOC:{table_name}.bbb]]")),
                        cell(&format!("[[AD_HOC:{table_name}.ccc]]")),
                    ],
                },
            ],
            ..Default::default()
        };
        let shape = deckcraft_model::Shape { kind: deckcraft_model::ShapeKind::Table(table), ..Default::default() };
        p.slides = vec![std::sync::Arc::new(deckcraft_model::Slide { shapes: vec![shape], ..Default::default() })];
        crate::recognize_variables(&mut p);
        p
    }

    /// A picture shape tagged as bound to an image variable named `name`.
    fn doc_with_image_variable(name: &str) -> Presentation {
        let mut p = deckcraft_model::defaults::new_presentation(None);
        let mut shape = deckcraft_model::Shape { kind: deckcraft_model::ShapeKind::Picture { fill: Default::default() }, ..Default::default() };
        crate::mark_image_variable(&mut shape, name);
        p.slides = vec![std::sync::Arc::new(deckcraft_model::Slide { shapes: vec![shape], ..Default::default() })];
        p
    }

    /// Same baseline skeleton `export_pptx_for_live_upload` always writes (see this module's own
    /// doc comment) — a document with zero variables is still exported as a recognizable LiveDocs
    /// file, it just has an empty `TenantOrigin` (no tenant is known for a plain local save).
    #[test]
    fn a_document_with_no_variables_still_gets_the_baseline_template_skeleton() {
        let p = deckcraft_model::defaults::new_presentation(None);
        let bytes = export_pptx_with_livedoc(&p).unwrap();
        let all = all_custom_xml(&bytes);
        assert!(all.contains("<Presentation UniqueId="), "{all}");
        assert!(all.contains("<TenantOrigin></TenantOrigin>"), "{all}");
        assert!(all.contains(r#"<VariableListDefinition name="Computed""#), "{all}");
        assert!(all.contains(r#"<VariableListDefinition name="System""#), "{all}");
        assert!(deckcraft_pptx::import(&bytes).is_ok());
    }

    #[test]
    fn a_document_with_one_variable_gets_a_full_ad_hoc_variable_list() {
        let p = doc_with_variable("my_name");
        let bytes = export_pptx_with_livedoc(&p).unwrap();
        let all = all_custom_xml(&bytes);
        assert!(all.contains(r#"<VariableListDefinition name="AD_HOC""#), "{all}");
        assert!(all.contains(r#"<Variable name="my_name" type="STRING""#), "{all}");
        assert!(all.contains("<DataSourceType>Ad_Hoc</DataSourceType>"), "{all}");
        assert!(all.contains(r#"Mapping="DataRecord/my_name""#), "{all}");
        assert!(all.contains(r#"variableListName="AD_HOC""#), "{all}");
        // The exported file should still re-import cleanly.
        assert!(deckcraft_pptx::import(&bytes).is_ok());
    }

    #[test]
    fn exporting_the_same_document_twice_produces_different_data_source_ids() {
        let p = doc_with_variable("my_name");
        let first = all_custom_xml(&export_pptx_with_livedoc(&p).unwrap());
        let second = all_custom_xml(&export_pptx_with_livedoc(&p).unwrap());
        let extract = |xml: &str| -> String {
            let start = xml.find("<VariableList UniqueId=\"").expect("has a VariableList part") + "<VariableList UniqueId=\"".len();
            xml[start..start + xml[start..].find('"').unwrap()].to_string()
        };
        assert_ne!(extract(&first), extract(&second), "two exports of the same document shouldn't share an id");
    }

    #[test]
    fn a_document_with_existing_kept_parts_does_not_collide_item_numbers() {
        let mut p = doc_with_variable("my_name");
        // A real pre-existing top-level customXml item needs its own relationship to be
        // discoverable on reimport (`deckcraft_pptx::read::kept_parts` walks relationships, not
        // the raw zip listing) — same shape [`item_part`] gives this module's own parts.
        p.kept_parts.push(KeptPart {
            name: "customXml/item1.xml".to_string(),
            xml: "<Existing/>".to_string(),
            rel_type: CUSTOM_XML_REL.to_string(),
            from_root: false,
            ..Default::default()
        });
        let bytes = export_pptx_with_livedoc(&p).unwrap();
        assert!(deckcraft_pptx::import(&bytes).is_ok());
        let imported = deckcraft_pptx::import(&bytes).unwrap();
        let existing_count = imported.kept_parts.iter().filter(|k| k.xml.contains("<Existing/>")).count();
        assert_eq!(existing_count, 1, "the pre-existing part should survive untouched, not be overwritten");
    }

    #[test]
    fn a_document_with_no_variables_still_gets_the_baseline_template_skeleton_for_live_upload() {
        let p = deckcraft_model::defaults::new_presentation(None);
        let bytes = export_pptx_for_live_upload(&p, "acme").unwrap();
        let all = all_custom_xml(&bytes);
        assert!(all.contains("<Presentation UniqueId="), "{all}");
        assert!(all.contains("<TenantOrigin>acme</TenantOrigin>"), "{all}");
        assert!(all.contains(r#"<VariableListDefinition name="Computed""#), "{all}");
        assert!(all.contains(r#"<VariableListDefinition name="System""#), "{all}");
        assert!(all.contains("<AllExternalAdhocVariableMappings/>"), "{all}");
        assert!(all.contains(r#"variableListName="Computed""#), "{all}");
        assert!(all.contains(r#"variableListName="System""#), "{all}");
        // AD_HOC's own (empty) skeleton is written unconditionally too, same as Computed/System
        // — a real FormService call against a zero-variable document still failed without this.
        assert!(all.contains(r#"variableListName="AD_HOC""#), "{all}");
        assert!(all.contains(r#"<VariableListDefinition name="AD_HOC""#), "{all}");
        assert_eq!(
            all.matches("<SourceType>XML File</SourceType>").count(),
            3,
            "Computed's, System's and AD_HOC's own DataSourceMapping should all use this SourceType, got: {all}"
        );
        assert!(deckcraft_pptx::import(&bytes).is_ok(), "should still re-import cleanly with the baseline skeleton attached");
    }

    #[test]
    fn a_document_with_a_variable_gets_both_the_baseline_skeleton_and_the_ad_hoc_list_for_live_upload() {
        let p = doc_with_variable("my_name");
        let bytes = export_pptx_for_live_upload(&p, "acme").unwrap();
        let all = all_custom_xml(&bytes);
        assert!(all.contains(r#"<VariableListDefinition name="AD_HOC""#), "{all}");
        assert!(all.contains(r#"<Variable name="my_name" type="STRING""#), "{all}");
        assert!(all.contains(r#"<VariableListDefinition name="Computed""#), "{all}");
        assert!(all.contains(r#"<VariableListDefinition name="System""#), "{all}");
        assert!(all.contains("<SdmcCustXmlId>"), "AD_HOC's rels entry should reference its SourceDataModel part, got: {all}");
        assert!(deckcraft_pptx::import(&bytes).is_ok());
    }

    /// The missing piece a real, independently-verified LiveDocs-authored sample
    /// (`000_Presentation2_v0.1`) confirmed: the shape holding the variable gets its own
    /// `Strauss.PersonalizationDefinition` custom-XML item, referenced from both the shape's own
    /// `p:custData` (round-trips back through `deckcraft_pptx::import` as `Shape::custom_xml_ref`)
    /// and the presentation-level relationships — matching `item22.xml`'s real structure exactly.
    #[test]
    fn the_shape_holding_a_variable_gets_a_personalization_definition_bound_to_the_ad_hoc_field() {
        let p = doc_with_variable("my_name");
        let bytes = export_pptx_for_live_upload(&p, "acme").unwrap();
        let all = all_custom_xml(&bytes);
        assert!(all.contains("Strauss.PersonalizationDefinition"), "{all}");
        assert!(all.contains("<pd:VariableReplacementDescriptor"), "{all}");
        // The same dataFieldID the AD_HOC list's own DataSourceInfo/DataSourceMapping use for
        // "my_name", and the same VariableList UniqueId — not independently fabricated ids.
        let field_id = {
            let start = all.find(r#"<Variable name="my_name" type="STRING" dataFieldId=""#).unwrap()
                + r#"<Variable name="my_name" type="STRING" dataFieldId=""#.len();
            all[start..start + all[start..].find('"').unwrap()].to_string()
        };
        assert!(all.contains(&format!(r#"dataFieldID="{field_id}""#)), "personalization should reference AD_HOC's own field id, got: {all}");

        let imported = deckcraft_pptx::import(&bytes).unwrap();
        let ref_part = imported.slides[0].shapes[0].custom_xml_ref.clone().expect("the bound shape should carry a custom_xml_ref");
        assert!(ref_part.starts_with("customXml/item"), "{ref_part}");
        assert!(deckcraft_pptx::export(&imported).is_ok(), "a re-exported, re-imported document should still round-trip");
    }

    /// The table shape holding a recognized dynamic-table binding gets a nested `TABLE`-type
    /// `AD_HOC` variable (one `Variable`/`TableInfo` per column, a `DataRecord/{table}/{col}`
    /// `RawMapping` each) and its own `Strauss.PersonalizationDefinition` carrying a
    /// `pd:DynamicTableDescriptor` — matches a real add-in-authored sample
    /// (`(New) Test_v1.0.PPTX`'s own `customXml/item38.xml`) structurally: the table's own field
    /// plus every column field in the outer `pd:DataReferenceList`, one `pd:BindTable` repeating
    /// row 1 (written `RepeatingRowIndexFrom`/`To="2"`, 1-indexed), and a `pd:DynamicTableColumn`
    /// per cell (empty for the header row, one `pd:DataReference` each for the repeating row).
    #[test]
    fn a_table_with_a_dynamic_table_binding_gets_a_dynamic_table_descriptor() {
        let p = doc_with_dynamic_table("zzzz");
        let bytes = export_pptx_for_live_upload(&p, "acme").unwrap();
        let all = all_custom_xml(&bytes);

        assert!(all.contains(r#"<Variable name="zzzz" type="TABLE""#), "{all}");
        assert!(all.contains(r#"isRepeatingTable="True""#), "{all}");
        assert!(all.contains(r#"<Variable name="aaa" type="STRING""#), "{all}");
        assert!(all.contains("<TableInfo>"), "{all}");
        assert!(all.contains(r#"Mapping="DataRecord/zzzz""#), "{all}");
        assert!(all.contains(r#"Mapping="DataRecord/zzzz/aaa""#), "{all}");
        assert!(all.contains(r#"Mapping="DataRecord/zzzz/bbb""#), "{all}");
        assert!(all.contains(r#"Mapping="DataRecord/zzzz/ccc""#), "{all}");

        assert!(all.contains("<pd:DynamicTableDescriptor"), "{all}");
        assert!(all.contains(r#"RepeatingRowIndexFrom="2" RepeatingRowIndexTo="2""#), "row 1 (0-indexed) is written 1-indexed: {all}");
        assert_eq!(all.matches("<pd:DynamicTableColumn ").count(), 6, "3 header cells + 3 bound cells: {all}");
        assert_eq!(all.matches(r#"rowIndex="0""#).count(), 3, "the header row's 3 cells are unbound: {all}");
        assert_eq!(all.matches(r#"rowIndex="1""#).count(), 3, "{all}");

        let imported = deckcraft_pptx::import(&bytes).unwrap();
        let deckcraft_model::ShapeKind::Table(_) = &imported.slides[0].shapes[0].kind else { panic!("still a table") };
        assert!(imported.slides[0].shapes[0].custom_xml_ref.is_some(), "the table shape should carry a custom_xml_ref");
        assert!(deckcraft_pptx::export(&imported).is_ok());
    }

    /// A picture tagged with [`crate::mark_image_variable`] gets a standalone
    /// `pd:ExternalContentPD` personalization item with *no* `AD_HOC` field at all — matches a real
    /// sample's own `customXml/item39.xml` (`(New) Test_v1.0.PPTX`): an empty outer
    /// `pd:DataReferenceList`, since the binding needs nothing from the variable list.
    #[test]
    fn a_picture_with_an_image_variable_gets_an_external_content_pd() {
        let p = doc_with_image_variable("my_image");
        let bytes = export_pptx_for_live_upload(&p, "acme").unwrap();
        let all = all_custom_xml(&bytes);

        assert!(all.contains("<pd:ExternalContentPD"), "{all}");
        assert!(all.contains(r#"name="my_image""#), "{all}");
        assert!(all.contains(r#"ExternalContentType="ExternalImage""#), "{all}");
        assert!(!all.contains(r#"<Variable name="my_image""#), "an image variable needs no AD_HOC field: {all}");

        let imported = deckcraft_pptx::import(&bytes).unwrap();
        assert_eq!(crate::image_variable_name(&imported.slides[0].shapes[0]), Some("my_image".to_string()));
        assert!(imported.slides[0].shapes[0].custom_xml_ref.is_some(), "the picture should carry a custom_xml_ref");
        assert!(deckcraft_pptx::export(&imported).is_ok());
    }

    /// A real sample's own `itemN.xml`/`itemPropsN.xml` pairing (same `N`, no gap between
    /// consecutive items) for a fresh document with no pre-existing `kept_parts` — the exact thing
    /// `000_Presentation2_v0.1`'s own `customXml/` directory has (ignoring that real sample's own
    /// edit-history gaps, e.g. its missing `item2.xml`, which aren't something to replicate).
    #[test]
    fn custom_xml_items_are_numbered_contiguously_from_one_with_matching_item_props_pairs() {
        let p = doc_with_variable("my_name");
        let bytes = export_pptx_for_live_upload(&p, "acme").unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        let mut item_ns: Vec<usize> =
            archive.file_names().filter_map(|n| n.strip_prefix("customXml/item")?.strip_suffix(".xml")?.parse().ok()).collect();
        item_ns.sort_unstable();
        let expected: Vec<usize> = (1..=item_ns.len()).collect();
        assert_eq!(item_ns, expected, "item numbers should start at 1 and be contiguous, no gaps");

        let mut prop_ns: Vec<usize> =
            archive.file_names().filter_map(|n| n.strip_prefix("customXml/itemProps")?.strip_suffix(".xml")?.parse().ok()).collect();
        prop_ns.sort_unstable();
        assert_eq!(prop_ns, item_ns, "every itemN.xml should pair with an itemPropsN.xml of the same N");

        for n in &item_ns {
            let mut rels = String::new();
            std::io::Read::read_to_string(&mut archive.by_name(&format!("customXml/_rels/item{n}.xml.rels")).unwrap(), &mut rels).unwrap();
            assert!(rels.contains(&format!("itemProps{n}.xml")), "item{n}.xml.rels should point at itemProps{n}.xml, got: {rels}");
        }
    }

    /// Every custom-XML content type a real LiveDocs-authored sample carries
    /// (`000_Presentation2_v0.1`) should appear somewhere in a live-upload export of a document
    /// with one variable, each exactly once except `VariableListDefinition`/`VariableList`/
    /// `DataSourceInfo`/`DataSourceMapping` (once per list: `AD_HOC`, `Computed`, `System`) and
    /// `SourceDataModel` (once, `AD_HOC` only) — and the shape's own slide should carry the
    /// `PersonalizationDefinition` relationship, not just the presentation-level one.
    #[test]
    fn every_expected_custom_xml_content_type_is_present_with_the_right_count() {
        let p = doc_with_variable("my_name");
        let bytes = export_pptx_for_live_upload(&p, "acme").unwrap();
        let all = all_custom_xml(&bytes);
        let count = |needle: &str| all.matches(needle).count();

        assert_eq!(count("<VariableListDefinition"), 3, "AD_HOC + Computed + System: {all}");
        assert_eq!(count("<VariableList "), 3, "{all}");
        assert_eq!(count("<DataSourceInfo>"), 3, "{all}");
        assert_eq!(count("<DataSourceMapping>"), 3, "{all}");
        assert_eq!(count("<SourceDataModel "), 1, "only AD_HOC gets one: {all}");
        assert_eq!(count("<VariableListCustXmlRels>"), 1, "{all}");
        assert_eq!(count("<AllExternalAdhocVariableMappings/>"), 1, "{all}");
        assert_eq!(count("<Presentation UniqueId="), 1, "{all}");
        assert_eq!(count("<TemplateEditing>"), 1, "{all}");
        assert_eq!(count("Strauss.PersonalizationDefinition"), 1, "{all}");
        assert!(all.contains("<pd:PersonalizationDefinition"), "{all}");

        // The personalization item must be wired from both the presentation's own relationships
        // (every other customXml item here gets this) and the bound shape's own slide relationship
        // (the thing a real LiveDocs session uses to know *which* shape owns the binding).
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        let mut pres_rels = String::new();
        std::io::Read::read_to_string(&mut archive.by_name("ppt/_rels/presentation.xml.rels").unwrap(), &mut pres_rels).unwrap();
        let item_names: Vec<String> = archive
            .file_names()
            .filter(|n| n.starts_with("customXml/item") && n.ends_with(".xml") && !n.contains("Props"))
            .map(str::to_string)
            .collect();
        let personalization_item = item_names
            .into_iter()
            .find(|n| {
                let mut s = String::new();
                std::io::Read::read_to_string(&mut archive.by_name(n).unwrap(), &mut s).unwrap();
                s.contains("Strauss.PersonalizationDefinition")
            })
            .expect("a customXml item should hold the PersonalizationDefinition");
        let item_name = personalization_item.strip_prefix("customXml/").unwrap();
        assert!(pres_rels.contains(item_name), "presentation.xml.rels should reference {item_name}, got: {pres_rels}");

        let mut slide_rels = String::new();
        std::io::Read::read_to_string(&mut archive.by_name("ppt/slides/_rels/slide1.xml.rels").unwrap(), &mut slide_rels).unwrap();
        assert!(slide_rels.contains(item_name), "slide1.xml.rels should also reference {item_name}, got: {slide_rels}");

        let mut slide1 = String::new();
        std::io::Read::read_to_string(&mut archive.by_name("ppt/slides/slide1.xml").unwrap(), &mut slide1).unwrap();
        assert!(slide1.contains("<p:custData"), "{slide1}");

        // `presentation.xml`'s own `p:custDataLst` lists every document-level item (identity,
        // variable lists, the rels index, AllExternalAdhocVariableMappings) except the
        // personalization one, which a shape already binds to — a real sample does the same, and
        // a real tenant's FormService can't find a template's identity metadata without this body
        // entry even though the relationship and part both exist.
        let mut pres_xml = String::new();
        std::io::Read::read_to_string(&mut archive.by_name("ppt/presentation.xml").unwrap(), &mut pres_xml).unwrap();
        assert!(pres_xml.contains("<p:custDataLst>"), "{pres_xml}");
        let cust_data_count = pres_xml.matches("<p:custData ").count();
        // Every document-level customXml item: Presentation, TemplateEditing, 5 AD_HOC parts, 4
        // Computed parts, 4 System parts, the rels index, AllExternalAdhocVariableMappings — 17
        // total, one less than the 18 customXml relationships (personalization excluded).
        assert_eq!(cust_data_count, 17, "{pres_xml}");
    }

    /// Every `customXml/itemN.xml` part's content, concatenated — item numbers aren't
    /// necessarily contiguous from 1 (these functions number past whatever the document already
    /// kept), so this reads the zip's own entry names rather than counting up.
    fn all_custom_xml(bytes: &[u8]) -> String {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("a valid zip");
        let names: Vec<String> = archive
            .file_names()
            .filter(|n| n.starts_with("customXml/item") && n.ends_with(".xml") && !n.contains("Props"))
            .map(str::to_string)
            .collect();
        let mut all = String::new();
        for name in names {
            let mut file = archive.by_name(&name).unwrap();
            let mut s = String::new();
            std::io::Read::read_to_string(&mut file, &mut s).unwrap();
            all += &s;
        }
        all
    }
}
