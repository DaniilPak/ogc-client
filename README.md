# ogc-client

[![CI](https://github.com/DaniilPak/ogc-client/actions/workflows/ci.yml/badge.svg)](https://github.com/DaniilPak/ogc-client/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/ogc-client.svg)](https://crates.io/crates/ogc-client)
[![docs.rs](https://docs.rs/ogc-client/badge.svg)](https://docs.rs/ogc-client)

A Rust client for [OGC](https://www.ogc.org/) web services: WMS, WMTS, WFS,
CSW and the OGC API family. It covers the same ground as Python's
[OWSLib](https://github.com/geopython/OWSLib): typed `GetCapabilities` parsing,
request builders, and response handling, with async HTTP through `reqwest`.

- **Typed capabilities** for every service: service identification, provider and
  contact, operations with their endpoints and parameters, layers with
  CRSs, bounding boxes, styles, legends and dimensions (time, elevation).
- **Request builders** that produce URLs without network access (`*_url`
  methods), so they're easy to test and to hand to other tools.
- **Axis order handled for you.** Bounding boxes passed to builders are always
  `(west, south, east, north)`. The client swaps axes where the protocol wants
  it: WMS 1.3.0 with EPSG:4326, WFS 1.1/2.0 URNs, GML 3 coordinates, WMTS corners.
- **GeoJSON first.** WFS `GetFeature` asks for GeoJSON when the server offers
  it and otherwise parses GML 2/3.1/3.2 into `geo-types` geometries and GeoJSON.
- **Tile math** for WMTS and OGC API – Tiles: the tile that covers a point,
  tile bounds, the tiles that cover a bbox.
- **OGC exceptions become errors.** `ows:ExceptionReport`,
  `ServiceExceptionReport` and OGC API problem JSON map to `Error::Service` /
  `Error::Api`, even when a server sends them with status 200 and an image
  content type.
- **Injectable HTTP client.** Bring your own `reqwest::Client` (proxies, TLS
  roots); add basic/bearer auth and custom headers. rustls is the only TLS
  backend. The `blocking` feature adds synchronous wrappers.

## Supported services

| Service | Versions | Operations | OWSLib equivalent |
|---|---|---|---|
| WMS | 1.1.1, 1.3.0 | GetCapabilities, GetMap, GetFeatureInfo, legend graphics | `owslib.wms.WebMapService` |
| WMTS | 1.0.0 | GetCapabilities, GetTile (KVP and RESTful `ResourceURL`), TileMatrixSet math | `owslib.wmts.WebMapTileService` |
| WFS | 1.1.0, 2.0.0 | GetCapabilities, GetFeature (bbox, FES filter, ids, sorting, paging, hits, stored query id), DescribeFeatureType | `owslib.wfs.WebFeatureService` |
| CSW | 2.0.2 | GetCapabilities, GetRecords (FES filter or CQL, POST or GET), GetRecordById | `owslib.csw.CatalogueServiceWeb` |
| OGC API – Features | 1.0 (Parts 1–3) | landing page, conformance, collections, items with paging, item, queryables, CQL2 `filter` | `owslib.ogcapi.features.Features` |
| OGC API – Records | 1.0 | catalogues, record search (`q`, bbox, datetime, filter), record | `owslib.ogcapi.records.Records` |
| OGC API – Tiles | 1.0 | tilesets, tileset metadata, tile matrix sets (JSON), tiles | (partial in OWSLib) |
| Filter Encoding | 1.1, 2.0 | comparison, like, null, between, bbox, ids, and/or/not | `owslib.fes` / `owslib.fes2` |

Not covered yet (OWSLib has them): WCS, WPS, SOS, WMC, ISO 19139 record
parsing beyond the main fields (the full XML is kept in `CswRecord::xml`),
CSW-T transactions, OGC API – Coverages/Processes/EDR.

## Install

```toml
[dependencies]
ogc-client = "0.1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

## Examples

### WMS

```rust,no_run
use ogc_client::wms::{GetFeatureInfo, GetMap, WebMapService, WmsVersion};

# async fn run() -> ogc_client::Result<()> {
let wms = WebMapService::connect(
    "https://basemap.nationalmap.gov/arcgis/services/USGSTopo/MapServer/WMSServer",
    WmsVersion::V1_3_0,
)
.await?;

for layer in wms.capabilities().named_layers() {
    println!("{:?} {:?} {:?}", layer.name, layer.title, layer.geographic_bbox);
}

// (west, south, east, north). For WMS 1.3.0 + EPSG:4326 the client sends
// BBOX=38,-78,39,-77 as the spec requires.
let map = GetMap::new(["0"], (-78.0, 38.0, -77.0, 39.0), "EPSG:4326", 512, 512)
    .format("image/png")
    .transparent(true);
let png = wms.get_map(&map).await?;
std::fs::write("map.png", &png.body).ok();

let info = wms
    .get_feature_info(&GetFeatureInfo::new(map, 256, 256).info_format("text/xml"))
    .await?;
println!("{}", info.text());
# Ok(()) }
```

### WMTS

```rust,no_run
use ogc_client::wmts::{GetTile, WebMapTileService};

# async fn run() -> ogc_client::Result<()> {
let wmts = WebMapTileService::connect(
    "https://gibs.earthdata.nasa.gov/wmts/epsg4326/best/1.0.0/WMTSCapabilities.xml",
)
.await?;
let tms = wmts.capabilities().tile_matrix_set("250m").unwrap();
let idx = tms.tile_at("3", 2.35, 48.85).unwrap(); // Paris, lon/lat (x/y)
// RESTful ResourceURL with the {Time} dimension filled in (default if omitted).
let tile = wmts
    .get_tile(
        &GetTile::new("MODIS_Terra_CorrectedReflectance_TrueColor", "250m", "3", idx.row, idx.col)
            .dimension("Time", "2024-06-01"),
    )
    .await?;
# Ok(()) }
```

### WFS

```rust,no_run
use ogc_client::fes::Filter;
use ogc_client::wfs::{GetFeature, WebFeatureService, WfsVersion};

# async fn run() -> ogc_client::Result<()> {
let wfs = WebFeatureService::connect("https://maps.dwd.de/geoserver/wfs", WfsVersion::V2_0_0).await?;

let req = GetFeature::new(["dwd:RBSN_T2m"])
    .bbox((5.0, 47.0, 15.0, 55.0), Some("EPSG:4326"))
    .filter(Filter::greater_than("TT", 20))
    .count(100);

println!("matches: {:?}", wfs.hits(&req).await?);

// GeoJSON if offered, GML otherwise — to_geojson() works for both.
let mut pages = wfs.pages(req, 50);
while let Some(page) = pages.next_page().await? {
    for f in page.to_geojson()?.features {
        println!("{:?}", f.id);
    }
}

let schema = wfs.describe_feature_type("dwd:RBSN_T2m").await?;
println!("geometry: {:?}", schema.geometry_property());
# Ok(()) }
```

### CSW

```rust,no_run
use ogc_client::csw::{CatalogueService, ElementSet, GetRecords};
use ogc_client::fes::Filter;

# async fn run() -> ogc_client::Result<()> {
let csw = CatalogueService::connect("https://demo.pycsw.org/cite/csw").await?;
let res = csw
    .get_records(
        &GetRecords::new()
            .filter(Filter::and([
                Filter::like("csw:AnyText", "*water*"),
                Filter::bbox(Some("ows:BoundingBox"), (-10.0, 40.0, 30.0, 70.0), Some("EPSG:4326")),
            ]))
            .element_set(ElementSet::Full)
            .max_records(10),
    )
    .await?;
println!("{:?} matched", res.matched);
for r in &res.records {
    println!("{:?}: {:?}", r.identifier, r.title);
}
# Ok(()) }
```

### OGC API – Features, Records, Tiles

```rust,no_run
use ogc_client::ogcapi::{Features, ItemsQuery, Records, RecordView, Tiles};

# async fn run() -> ogc_client::Result<()> {
let api = Features::new("https://demo.pygeoapi.io/master")?;
let collections = api.collections().await?;
let page = api
    .items("lakes", &ItemsQuery::new().limit(10).bbox(-100.0, 30.0, -60.0, 60.0))
    .await?;
let next = api.next_page(&page).await?; // follows rel="next"

let records = Records::new("https://demo.pygeoapi.io/master")?;
let hits = records.records("dutch-metadata", &ItemsQuery::new().q("water")).await?;
for f in &hits.features.features {
    println!("{:?}", RecordView(f).title());
}

let tiles = Tiles::new("https://demo.pygeoapi.io/master")?;
let ts = tiles.collection_tileset("lakes", "WebMercatorQuad").await?;
let tms = tiles.tile_matrix_set(&ts).await?.to_tile_matrix_set();
let idx = tms.tile_at("4", 261_845.0, 6_250_566.0).unwrap();
let mvt = tiles.get_tile(&ts, "4", idx.row, idx.col).await?;
# Ok(()) }
```

### Offline parsing, custom client, blocking

```rust,no_run
use ogc_client::wms::WmsCapabilities;
use ogc_client::{Auth, Client};

# fn run() -> ogc_client::Result<()> {
let caps = WmsCapabilities::parse(&std::fs::read("capabilities.xml").unwrap())?;

let client = Client::from_reqwest(reqwest::Client::new())
    .with_auth(Auth::Basic { username: "me".into(), password: Some("secret".into()) })
    .with_header("X-Api-Key", "...")?;
# Ok(()) }
```

With `features = ["blocking"]`, `ogc_client::blocking::{WebMapService,
WebMapTileService, WebFeatureService, CatalogueService, Features}` offer the
same calls without `async`.

## Axis order in one paragraph

EPSG says EPSG:4326 is latitude/longitude. WMS 1.1.1, `CRS:84`, GeoJSON and
most humans say longitude/latitude. This crate takes and returns x/y
(longitude first) everywhere. It converts at the protocol boundary using
OWSLib's table of y/x EPSG codes. URN and `http://www.opengis.net/def/crs`
forms follow EPSG. The short `EPSG:4326` form in GML is ambiguous (GeoServer
writes lon/lat, MapServer WFS 1.1 writes lat/lon), so the order is detected
from the coordinate ranges. Override per CRS with `Crs::with_axis_order` or
send numbers verbatim with `AxisPolicy::AsGiven`.

## Testing

`cargo test` uses real documents saved once from NASA GIBS, USGS National Map,
the MapServer demo, DWD GeoServer, PDOK (INSPIRE cadastral parcels), the
pycsw demo and the pygeoapi demo (`tests/fixtures`). Async request paths run
against a loopback server that replays those fixtures, so no test touches the
internet. Live smoke tests against the public servers run with
`cargo test --test live -- --ignored`.

## Credits and license

The design follows [OWSLib](https://github.com/geopython/OWSLib) (BSD-3-Clause)
by the geopython community. Thanks to its authors. The table of y/x EPSG codes in
`src/crs.rs` is copied from OWSLib's `owslib/crs.py`; its BSD notice is in
[`LICENSE-BSD-3-Clause-OWSLib`](LICENSE-BSD-3-Clause-OWSLib).

**This is an independent project. It is not affiliated with, or endorsed by,
OWSLib, geopython or the OGC.**

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or
[MIT](LICENSE-MIT) at your option, plus the BSD-3-Clause notice above for
the ported table. SPDX: `(MIT OR Apache-2.0) AND BSD-3-Clause`.
