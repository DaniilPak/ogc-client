mod common;

use common::fixture;
use ogc_client::ogcapi::tiles::{TileMatrixSetDef, TileMatrixSetList, TileSet, TileSetList};
use ogc_client::ogcapi::{
    Collection, Collections, Conformance, Features, ItemsPage, ItemsQuery, LandingPage, LinkExt,
    RecordView,
};
use ogc_client::tms::TileIndex;

#[test]
fn landing_conformance_collections() {
    let lp: LandingPage = serde_json::from_slice(&fixture("oapi_landing.json")).unwrap();
    assert!(lp.title.as_deref().unwrap().contains("pygeoapi"));
    assert!(lp.links.find_rel("conformance").is_some());
    assert!(lp.links.find_rel("data").is_some());

    let conf: Conformance = serde_json::from_slice(&fixture("oapi_conformance.json")).unwrap();
    assert!(conf.has("ogcapi-features-1/1.0/conf/core"));
    assert!(conf.has("ogcapi-records-1/1.0/conf"));
    assert!(conf.has("ogcapi-tiles-1/1.0/conf"));

    let cols: Collections = serde_json::from_slice(&fixture("oapi_collections.json")).unwrap();
    assert!(cols.collections.len() > 10);
    let lakes = cols.collections.iter().find(|c| c.id == "lakes").unwrap();
    assert!(lakes.has_items());
    assert!(lakes.has_tiles());
    let records: Vec<_> = cols
        .collections
        .iter()
        .filter(|c| c.item_type.as_deref() == Some("record"))
        .collect();
    assert!(records.iter().any(|c| c.id == "dutch-metadata"));
}

#[test]
fn collection_extent() {
    let c: Collection = serde_json::from_slice(&fixture("oapi_lakes.json")).unwrap();
    assert_eq!(c.id, "lakes");
    assert_eq!(c.keywords, vec!["lakes".to_string()]);
    let ext = c.extent.unwrap();
    assert_eq!(
        ext.spatial.unwrap().bbox[0],
        vec![-180.0, -90.0, 180.0, 90.0]
    );
    let t = ext.temporal.unwrap();
    assert!(t.interval[0][0].is_some() && t.interval[0][1].is_none());
    assert_eq!(
        c.storage_crs.as_deref(),
        Some("http://www.opengis.net/def/crs/OGC/1.3/CRS84")
    );
}

#[test]
fn items_page_and_paging_links() {
    let page = ItemsPage::parse(&fixture("oapi_lakes_items.json")).unwrap();
    assert_eq!(page.len(), 3);
    assert_eq!(page.number_matched, Some(25));
    assert_eq!(page.number_returned, Some(3));
    assert!(page.next_url().unwrap().contains("offset=3"));
    let f = &page.features.features[0];
    assert!(f.geometry.is_some());
    assert!(f.property("name").is_some());
    // geo-types interop
    let g: geo_types::Geometry<f64> = f.geometry.clone().unwrap().try_into().unwrap();
    assert!(matches!(
        g,
        geo_types::Geometry::Polygon(_) | geo_types::Geometry::MultiPolygon(_)
    ));
}

#[test]
fn records_view() {
    let page = ItemsPage::parse(&fixture("oapi_records_items.json")).unwrap();
    assert_eq!(page.len(), 2);
    assert!(page.number_matched.unwrap() > 100);
    let r = RecordView(&page.features.features[0]);
    assert_eq!(
        r.id().as_deref(),
        Some("35149dfb-31d3-431c-a8bc-12a4034dac48")
    );
    assert_eq!(r.title(), Some("Kaartboeck 1635"));
    assert_eq!(r.type_(), Some("dataset"));
    assert!(r.description().unwrap().contains("kaartboeken"));
    assert!(!r.contacts().is_empty());
    assert!(r.time().is_some());
}

#[test]
fn items_url() {
    let api = Features::new("https://demo.pygeoapi.io/master").unwrap();
    let url = api
        .items_url(
            "lakes",
            &ItemsQuery::new()
                .limit(10)
                .bbox(-10.0, 40.0, 10.0, 60.0)
                .filter("name LIKE 'Lake%'"),
        )
        .unwrap();
    assert_eq!(
        url.as_str(),
        "https://demo.pygeoapi.io/master/collections/lakes/items?limit=10&bbox=-10%2C40%2C10%2C60&filter=name+LIKE+%27Lake%25%27&filter-lang=cql2-text"
    );
}

#[test]
fn tiles_documents_and_math() {
    let list: TileSetList = serde_json::from_slice(&fixture("oapi_lakes_tiles.json")).unwrap();
    assert_eq!(list.tilesets.len(), 1);
    assert_eq!(
        list.tilesets[0].tile_matrix_set_id(),
        Some("WebMercatorQuad")
    );
    assert_eq!(list.tilesets[0].data_type.as_deref(), Some("vector"));

    let ts: TileSet = serde_json::from_slice(&fixture("oapi_lakes_tileset.json")).unwrap();
    let url = ts.tile_url("3", 2, 4, None).unwrap();
    assert_eq!(
        url.as_str(),
        "https://demo.pygeoapi.io/master/collections/lakes/tiles/WebMercatorQuad/3/2/4?f=mvt"
    );
    assert!(ts.contains_tile("3", 2, 4));

    let tmsl: TileMatrixSetList =
        serde_json::from_slice(&fixture("oapi_tilematrixsets.json")).unwrap();
    assert_eq!(tmsl.tile_matrix_sets.len(), 2);

    let def: TileMatrixSetDef =
        serde_json::from_slice(&fixture("oapi_tms_webmercator.json")).unwrap();
    let tms = def.to_tile_matrix_set();
    assert_eq!(tms.identifier, "WebMercatorQuad");
    assert!(tms.tile_matrices.len() > 15);
    let z1 = tms.matrix("1").unwrap();
    assert_eq!((z1.matrix_width, z1.matrix_height), (2, 2));
    // Paris in EPSG:3857 at z3 → x=4, y=2 (the XYZ scheme agrees).
    assert_eq!(
        tms.tile_at("3", 261_845.0, 6_250_566.0),
        Some(TileIndex { row: 2, col: 4 })
    );
    // The same answer as the WMTS GoogleMapsCompatible set from USGS.
    let wmts = ogc_client::wmts::WmtsCapabilities::parse(&fixture("wmts_usgs.xml")).unwrap();
    let g = wmts.tile_matrix_set("GoogleMapsCompatible").unwrap();
    assert_eq!(
        g.tile_at("10", -8575605.0, 4707174.0),
        tms.tile_at("10", -8575605.0, 4707174.0)
    );
}
