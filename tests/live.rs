//! Live smoke tests against public servers. Ignored by default:
//! `cargo test --test live -- --ignored`

use ogc_client::csw::{CatalogueService, GetRecords};
use ogc_client::ogcapi::{Features, ItemsQuery, Tiles};
use ogc_client::wfs::{GetFeature, WebFeatureService, WfsVersion};
use ogc_client::wms::{GetMap, WebMapService, WmsVersion};
use ogc_client::wmts::{GetTile, WebMapTileService};

#[tokio::test]
#[ignore]
async fn live_wms_usgs() {
    let wms = WebMapService::connect(
        "https://basemap.nationalmap.gov/arcgis/services/USGSTopo/MapServer/WMSServer",
        WmsVersion::V1_3_0,
    )
    .await
    .unwrap();
    let img = wms
        .get_map(
            &GetMap::new(["0"], (-78.0, 38.0, -77.0, 39.0), "EPSG:4326", 64, 64)
                .format("image/png"),
        )
        .await
        .unwrap();
    assert!(img.body.starts_with(b"\x89PNG"));
}

#[tokio::test]
#[ignore]
async fn live_wmts_usgs() {
    let wmts = WebMapTileService::connect(
        "https://basemap.nationalmap.gov/arcgis/rest/services/USGSTopo/MapServer/WMTS/1.0.0/WMTSCapabilities.xml",
    )
    .await
    .unwrap();
    let tile = wmts
        .get_tile(&GetTile::new("USGSTopo", "GoogleMapsCompatible", "3", 2, 2))
        .await
        .unwrap();
    assert!(!tile.body.is_empty());
}

#[tokio::test]
#[ignore]
async fn live_wfs_mapserver() {
    let wfs =
        WebFeatureService::connect("https://demo.mapserver.org/cgi-bin/wfs", WfsVersion::V2_0_0)
            .await
            .unwrap();
    let resp = wfs
        .get_feature(&GetFeature::new(["ms:cities"]).count(3))
        .await
        .unwrap();
    assert_eq!(resp.to_geojson().unwrap().features.len(), 3);
}

#[tokio::test]
#[ignore]
async fn live_csw_pycsw() {
    let csw = CatalogueService::connect("https://demo.pycsw.org/cite/csw")
        .await
        .unwrap();
    let r = csw
        .get_records(&GetRecords::new().max_records(2))
        .await
        .unwrap();
    assert_eq!(r.records.len(), 2);
}

#[tokio::test]
#[ignore]
async fn live_ogcapi_pygeoapi() {
    let api = Features::new("https://demo.pygeoapi.io/master").unwrap();
    let page = api
        .items("lakes", &ItemsQuery::new().limit(2))
        .await
        .unwrap();
    assert_eq!(page.len(), 2);
    let next = api.next_page(&page).await.unwrap().unwrap();
    assert_eq!(next.len(), 2);
    let tiles = Tiles::new("https://demo.pygeoapi.io/master").unwrap();
    let list = tiles.collection_tilesets("lakes").await.unwrap();
    assert!(!list.tilesets.is_empty());
}
