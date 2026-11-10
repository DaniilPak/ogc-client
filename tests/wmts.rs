mod common;

use common::fixture;
use ogc_client::Client;
use ogc_client::tms::TileIndex;
use ogc_client::wmts::{GetTile, TileEncoding, WebMapTileService, WmtsCapabilities};

#[test]
fn usgs_capabilities_and_math() {
    let caps = WmtsCapabilities::parse(&fixture("wmts_usgs.xml")).unwrap();
    assert_eq!(caps.version, "1.0.0");
    let layer = caps.layer("USGSTopo").unwrap();
    assert_eq!(layer.default_style(), Some("default"));
    assert!(layer.has_tile_matrix_set("default028mm"));
    assert!(layer.has_tile_matrix_set("GoogleMapsCompatible"));
    assert_eq!(layer.resource_urls.len(), 1);
    let wgs = layer.wgs84_bbox.as_ref().unwrap();
    assert!(wgs.minx < -179.0);

    let gm = caps.operation("GetTile").unwrap();
    assert!(gm.get_url_for_encoding("KVP").unwrap().ends_with("WMTS?"));

    let tms = caps.tile_matrix_set("GoogleMapsCompatible").unwrap();
    assert!(tms.crs.contains("3857"));
    let z0 = &tms.tile_matrices[0];
    assert_eq!((z0.matrix_width, z0.matrix_height), (1, 1));
    assert!((z0.top_left.0 + 20037508.34).abs() < 1.0);
    // 256 px * pixel size at z0 spans the whole Web Mercator world.
    let (sx, _) = z0.tile_span(tms.meters_per_unit());
    assert!((sx - 40075016.68).abs() < 10.0, "{sx}");
    // Washington DC at z10.
    let idx = tms.tile_at("10", -8575605.0, 4707174.0).unwrap();
    assert_eq!(idx, TileIndex { row: 391, col: 292 });
    let (minx, miny, maxx, maxy) = tms.tile_bbox("10", idx).unwrap();
    assert!(minx <= -8575605.0 && maxx >= -8575605.0);
    assert!(miny <= 4707174.0 && maxy >= 4707174.0);
}

#[test]
fn gibs_crs84_and_dimensions() {
    let caps = WmtsCapabilities::parse(&fixture("wmts_gibs.xml")).unwrap();
    assert!(caps.layers.len() > 1000);
    assert!(caps.tile_matrix_sets.len() >= 5);
    let tms = caps.tile_matrix_set("2km").unwrap();
    assert!(tms.crs.contains("CRS84"));
    let z0 = &tms.tile_matrices[0];
    assert_eq!(z0.top_left, (-180.0, 90.0));
    // Geographic: pixel size in degrees.
    let px = z0.pixel_size(tms.meters_per_unit());
    assert!(px > 0.1 && px < 1.0, "{px}");
    let layer = caps.layer("MERRA2_2m_Air_Temperature_Monthly").unwrap();
    let time = &layer.dimensions[0];
    assert_eq!(time.identifier, "Time");
    assert!(time.default.is_some());
    assert!(time.values.len() >= 2);
    assert!(
        layer
            .resource_urls
            .iter()
            .any(|r| r.template.contains("{Time}"))
    );
}

#[test]
fn pdok_rd_new_tile_matrix_set() {
    let caps = WmtsCapabilities::parse(&fixture("wmts_pdok.xml")).unwrap();
    assert_eq!(caps.layers.len(), 5);
    let tms = caps.tile_matrix_set("EPSG:28992").unwrap();
    // RD New is x/y; the top-left corner must not be swapped.
    let tl = tms.tile_matrices[0].top_left;
    assert!(tl.0 < 0.0 && tl.1 > 800_000.0, "{tl:?}");
    // Amsterdam (RD) is in the matrix at every level.
    for m in &tms.tile_matrices {
        assert!(
            m.tile_at(121_000.0, 487_000.0, 1.0).is_some(),
            "{}",
            m.identifier
        );
    }
}

#[test]
fn tile_urls_rest_and_kvp() {
    let wmts = WebMapTileService::from_capabilities(
        Client::new(),
        "https://basemap.nationalmap.gov/arcgis/rest/services/USGSTopo/MapServer/WMTS/1.0.0/WMTSCapabilities.xml",
        &fixture("wmts_usgs.xml"),
    )
    .unwrap();
    let req = GetTile::new("USGSTopo", "GoogleMapsCompatible", "10", 391, 292);
    let rest = wmts.get_tile_url(&req).unwrap();
    assert_eq!(
        rest.as_str(),
        "https://basemap.nationalmap.gov/arcgis/rest/services/USGSTopo/MapServer/WMTS/tile/1.0.0/USGSTopo/default/GoogleMapsCompatible/10/391/292"
    );
    let kvp = wmts
        .get_tile_url(&req.clone().encoding(TileEncoding::Kvp))
        .unwrap();
    let s = kvp.as_str();
    assert!(s.starts_with(
        "https://basemap.nationalmap.gov/arcgis/rest/services/USGSTopo/MapServer/WMTS?"
    ));
    for p in [
        "REQUEST=GetTile",
        "LAYER=USGSTopo",
        "STYLE=default",
        "TILEMATRIXSET=GoogleMapsCompatible",
        "TILEMATRIX=10",
        "TILEROW=391",
        "TILECOL=292",
    ] {
        assert!(s.contains(p), "{p} in {s}");
    }
    assert!(
        wmts.get_tile_url(&GetTile::new("USGSTopo", "nope", "1", 0, 0))
            .is_err()
    );
}

#[test]
fn gibs_time_template() {
    let wmts = WebMapTileService::from_capabilities(
        Client::new(),
        "https://gibs.earthdata.nasa.gov/wmts/epsg4326/best/1.0.0/WMTSCapabilities.xml",
        &fixture("wmts_gibs.xml"),
    )
    .unwrap();
    let req = GetTile::new("MERRA2_2m_Air_Temperature_Monthly", "2km", "1", 0, 1)
        .dimension("time", "2020-01-01");
    let url = wmts.get_tile_url(&req).unwrap();
    assert_eq!(
        url.as_str(),
        "https://gibs.earthdata.nasa.gov/wmts/epsg4326/best/MERRA2_2m_Air_Temperature_Monthly/default/2020-01-01/2km/1/0/1.png"
    );
    // Without an explicit time the default is used.
    let def = wmts
        .get_tile_url(&GetTile::new(
            "MERRA2_2m_Air_Temperature_Monthly",
            "2km",
            "1",
            0,
            1,
        ))
        .unwrap();
    let layer = wmts
        .capabilities()
        .layer("MERRA2_2m_Air_Temperature_Monthly")
        .unwrap();
    assert!(
        def.as_str()
            .contains(layer.dimensions[0].default.as_deref().unwrap())
    );
}
