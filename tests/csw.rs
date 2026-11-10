mod common;

use common::fixture;
use ogc_client::Client;
use ogc_client::csw::{CatalogueService, CswCapabilities, ElementSet, GetRecords, RecordsResponse};

#[test]
fn pycsw_capabilities() {
    let caps = CswCapabilities::parse(&fixture("csw_pycsw.xml")).unwrap();
    assert_eq!(caps.version, "2.0.2");
    assert_eq!(caps.identification.service_type.as_deref(), Some("CSW"));
    assert!(caps.identification.versions.contains(&"2.0.2".to_string()));
    assert_eq!(caps.provider.contact.city.as_deref(), Some("City"));
    assert_eq!(
        caps.provider.contact.role.as_deref(),
        Some("pointOfContact")
    );
    let gr = caps.operation("GetRecords").unwrap();
    assert!(gr.post_url().is_some());
    assert_eq!(
        gr.parameters.get("ElementSetName").unwrap(),
        &vec!["brief".to_string(), "full".into(), "summary".into()]
    );
    assert!(caps.queryables().contains(&"csw:AnyText".to_string()));
    assert!(
        caps.output_schemas()
            .contains(&"http://www.opengis.net/cat/csw/2.0.2".to_string())
    );
    assert_eq!(
        caps.constraints.get("MaxRecordDefault").unwrap(),
        &vec!["10".to_string()]
    );
    assert!(caps.spatial_operators.contains(&"BBOX".to_string()));
    assert!(caps.comparison_operators.contains(&"Like".to_string()));
}

#[test]
fn get_records_full() {
    let r = RecordsResponse::parse(&fixture("csw_getrecords.xml")).unwrap();
    assert_eq!(r.matched, Some(12));
    assert_eq!(r.returned, Some(3));
    assert_eq!(r.next_record, Some(4));
    assert_eq!(r.element_set.as_deref(), Some("full"));
    assert_eq!(r.records.len(), 3);
    let first = &r.records[0];
    assert_eq!(first.record_type, "Record");
    assert_eq!(
        first.identifier.as_deref(),
        Some("urn:uuid:19887a8a-f6b0-4a63-ae56-7fba0e17801f")
    );
    assert_eq!(first.title.as_deref(), Some("Lorem ipsum"));
    assert_eq!(first.subjects, vec!["Tourism--Greece".to_string()]);
    assert_eq!(first.spatial.as_deref(), Some("GR-22"));
    assert!(first.abstract_.as_deref().unwrap().starts_with("Quisque"));
    let second = r
        .record("urn:uuid:1ef30a8b-876d-4828-9246-c37ab4510bbd")
        .unwrap();
    let bb = second.bbox.as_ref().unwrap();
    assert_eq!(bb.crs.as_deref(), Some("urn:x-ogc:def:crs:EPSG:6.11:4326"));
    // lat/lon in the document; to_xy gives lon/lat.
    assert_eq!(bb.to_xy(true), (13.754, 60.042, 17.920, 68.410));
    assert!(first.xml.contains("Lorem ipsum"));
}

#[test]
fn get_records_summary_and_by_id() {
    let r = RecordsResponse::parse(&fixture("csw_getrecords10.xml")).unwrap();
    assert_eq!(r.records.len(), 10);
    assert!(r.records.iter().all(|x| x.record_type == "SummaryRecord"));
    assert_eq!(r.next_record, Some(11));
    let one = RecordsResponse::parse(&fixture("csw_getrecordbyid.xml")).unwrap();
    assert_eq!(one.records.len(), 1);
    assert_eq!(one.records[0].title.as_deref(), Some("Lorem ipsum"));
}

#[test]
fn urls() {
    let csw = CatalogueService::from_capabilities(
        Client::new(),
        "https://demo.pycsw.org/cite/csw",
        &fixture("csw_pycsw.xml"),
    )
    .unwrap();
    let url = csw
        .get_records_url(
            &GetRecords::new()
                .cql("dc:title like '%lorem%'")
                .max_records(3),
        )
        .unwrap();
    let s = url.as_str();
    assert!(s.starts_with("https://demo.pycsw.org/cite/csw?"));
    assert!(s.contains("CONSTRAINTLANGUAGE=CQL_TEXT"));
    assert!(s.contains("MAXRECORDS=3"));
    let url = csw
        .get_record_by_id_url(&["a", "b"], ElementSet::Brief, None)
        .unwrap();
    assert!(url.as_str().contains("ID=a%2Cb"));
    assert!(url.as_str().contains("ELEMENTSETNAME=brief"));
}
