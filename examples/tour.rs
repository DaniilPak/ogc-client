//! A quick tour against public demo servers.
//!
//! `cargo run --example tour`

use ogc_client::csw::{CatalogueService, GetRecords};
use ogc_client::fes::Filter;
use ogc_client::ogcapi::{Features, ItemsQuery};
use ogc_client::wfs::{GetFeature, WebFeatureService, WfsVersion};
use ogc_client::wms::{GetMap, WebMapService, WmsVersion};
use ogc_client::wmts::{GetTile, WebMapTileService};

#[tokio::main]
async fn main() -> ogc_client::Result<()> {
    // WMS: capabilities + a map image (bbox is always west, south, east, north).
    let wms = WebMapService::connect(
        "https://basemap.nationalmap.gov/arcgis/services/USGSTopo/MapServer/WMSServer",
        WmsVersion::V1_3_0,
    )
    .await?;
    println!("WMS: {:?}", wms.capabilities().service.title);
    let png = wms
        .get_map(
            &GetMap::new(["0"], (-78.0, 38.0, -77.0, 39.0), "EPSG:4326", 256, 256)
                .format("image/png"),
        )
        .await?;
    println!("  GetMap: {} bytes", png.body.len());

    // WMTS: find the tile covering a point and fetch it.
    let wmts = WebMapTileService::connect(
        "https://basemap.nationalmap.gov/arcgis/rest/services/USGSTopo/MapServer/WMTS/1.0.0/WMTSCapabilities.xml",
    )
    .await?;
    let tms = wmts
        .capabilities()
        .tile_matrix_set("GoogleMapsCompatible")
        .expect("tile matrix set");
    let idx = tms.tile_at("10", -8575605.0, 4707174.0).expect("inside");
    let tile = wmts
        .get_tile(&GetTile::new(
            "USGSTopo",
            "GoogleMapsCompatible",
            "10",
            idx.row,
            idx.col,
        ))
        .await?;
    println!("WMTS tile {idx:?}: {} bytes", tile.body.len());

    // WFS: features as GeoJSON (or GML converted to GeoJSON).
    let wfs =
        WebFeatureService::connect("https://demo.mapserver.org/cgi-bin/wfs", WfsVersion::V2_0_0)
            .await?;
    let fc = wfs
        .get_feature(
            &GetFeature::new(["ms:cities"])
                .filter(Filter::greater_than("POPULATION", 10_000_000))
                .count(5),
        )
        .await?
        .to_geojson()?;
    for f in &fc.features {
        println!("WFS: {:?}", f.property("NAME"));
    }

    // CSW: search a catalogue.
    let csw = CatalogueService::connect("https://demo.pycsw.org/cite/csw").await?;
    let res = csw.get_records(&GetRecords::new().max_records(3)).await?;
    println!("CSW: {:?} records matched", res.matched);

    // OGC API - Features with paging.
    let api = Features::new("https://demo.pygeoapi.io/master")?;
    let page = api.items("lakes", &ItemsQuery::new().limit(5)).await?;
    println!(
        "OGC API: {} of {:?} lakes, next: {:?}",
        page.len(),
        page.number_matched,
        page.next_url()
    );
    Ok(())
}
