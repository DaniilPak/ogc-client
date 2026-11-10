//! End-to-end tests of the async request paths against a loopback server
//! that replays the saved fixtures (no internet access).

mod common;

use common::{Route, fixture, route, serve};
use ogc_client::csw::{CatalogueService, ElementSet, GetRecords};
use ogc_client::fes::Filter;
use ogc_client::ogcapi::{Features, ItemsQuery, RecordView, Records, Tiles};
use ogc_client::wfs::{FeatureResponse, GetFeature, OutputFormat, WebFeatureService, WfsVersion};
use ogc_client::wms::{GetMap, WebMapService, WmsVersion};
use ogc_client::wmts::{GetTile, TileEncoding, WebMapTileService};
use ogc_client::{Auth, Client, Error};

/// Fixture with its real host rewritten to the mock server placeholder.
fn rehost(name: &str, hosts: &[&str]) -> Vec<u8> {
    let mut s = String::from_utf8(fixture(name)).unwrap();
    for h in hosts {
        s = s.replace(h, "{BASE}");
    }
    s.into_bytes()
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake";

#[tokio::test]
async fn wms_connect_getmap_and_exceptions() {
    let usgs = [
        "https://basemap.nationalmap.gov:443",
        "https://basemap.nationalmap.gov",
    ];
    let server = serve(vec![
        route(
            &["request=getcapabilities", "service=wms"],
            "text/xml",
            rehost("wms130_usgs.xml", &usgs),
        ),
        // A server returning an exception with status 200 and an image MIME type.
        route(
            &["request=getmap", "format=image/jpeg"],
            "image/jpeg",
            fixture("wms_exception.xml"),
        ),
        route(&["request=getmap"], "image/png", PNG.to_vec()),
    ])
    .await;
    let client = Client::new()
        .with_auth(Auth::Bearer("t0ken".into()))
        .with_header("X-Test", "1")
        .unwrap();
    let wms = WebMapService::connect_with(
        client,
        &format!(
            "{}/arcgis/services/USGSTopo/MapServer/WMSServer",
            server.base
        ),
        WmsVersion::V1_3_0,
    )
    .await
    .unwrap();
    assert_eq!(wms.version(), WmsVersion::V1_3_0);
    let img = wms
        .get_map(&GetMap::new(
            ["0"],
            (-78.0, 38.0, -77.0, 39.0),
            "EPSG:4326",
            64,
            64,
        ))
        .await
        .unwrap();
    assert_eq!(img.content_type.as_deref(), Some("image/png"));
    assert_eq!(&img.body[..], PNG);
    let err = wms
        .get_map(
            &GetMap::new(["0"], (-78.0, 38.0, -77.0, 39.0), "EPSG:4326", 64, 64)
                .format("image/jpeg"),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Service(_)), "{err:?}");

    let reqs = server.requests();
    assert!(reqs[0].line.contains("VERSION=1.3.0"));
    assert!(
        reqs[1].line.contains("BBOX=38,-78,39,-77"),
        "{}",
        reqs[1].line
    );
    let h = reqs[1].headers.to_ascii_lowercase();
    assert!(h.contains("authorization: bearer t0ken"));
    assert!(h.contains("x-test: 1"));
}

#[tokio::test]
async fn http_status_errors() {
    let mut not_found = route(
        &["/collections/missing"],
        "application/json",
        br#"{"code":"NotFound","description":"Collection not found"}"#.to_vec(),
    );
    not_found.status = 404;
    let mut boom = route(&["/conformance"], "text/plain", b"boom".to_vec());
    boom.status = 500;
    let server = serve(vec![not_found, boom]).await;
    let api = Features::new(&server.base).unwrap();
    match api.collection("missing").await.unwrap_err() {
        Error::Api { code, description } => {
            assert_eq!(code, "NotFound");
            assert_eq!(description, "Collection not found");
        }
        e => panic!("{e:?}"),
    }
    match api.conformance().await.unwrap_err() {
        Error::Status { status, body, .. } => {
            assert_eq!(status, 500);
            assert_eq!(body, "boom");
        }
        e => panic!("{e:?}"),
    }
}

#[tokio::test]
async fn wmts_rest_and_kvp_tiles() {
    let host = ["https://basemap.nationalmap.gov"];
    let server = serve(vec![
        route(
            &["wmtscapabilities.xml"],
            "text/xml",
            rehost("wmts_usgs.xml", &host),
        ),
        route(
            &["/tile/1.0.0/usgstopo/default/googlemapscompatible/3/2/1"],
            "image/png",
            PNG.to_vec(),
        ),
        route(
            &["request=gettile", "tilerow=2", "tilecol=1"],
            "image/png",
            PNG.to_vec(),
        ),
    ])
    .await;
    let wmts = WebMapTileService::connect(&format!(
        "{}/arcgis/rest/services/USGSTopo/MapServer/WMTS/1.0.0/WMTSCapabilities.xml",
        server.base
    ))
    .await
    .unwrap();
    let req = GetTile::new("USGSTopo", "GoogleMapsCompatible", "3", 2, 1);
    assert_eq!(&wmts.get_tile(&req).await.unwrap().body[..], PNG);
    let kvp = wmts
        .get_tile(&req.encoding(TileEncoding::Kvp))
        .await
        .unwrap();
    assert_eq!(&kvp.body[..], PNG);
    assert_eq!(server.requests().len(), 3);
}

#[tokio::test]
async fn wfs_geojson_gml_hits_paging_and_schema() {
    let dwd = ["https://maps.dwd.de"];
    let ms = ["https://demo.mapserver.org"];
    let gml_page_1 = rehost("wfs200_mapserver_getfeature.xml", &ms);
    let server = serve(vec![
        route(&["/geoserver/wfs", "request=getcapabilities"], "text/xml", rehost("wfs200_dwd.xml", &dwd)),
        route(
            &["/geoserver/wfs", "request=getfeature", "outputformat=application/json"],
            "application/json",
            fixture("wfs200_dwd_getfeature.json"),
        ),
        route(&["/cgi-bin/wfs", "request=getcapabilities"], "text/xml", rehost("wfs200_mapserver.xml", &ms)),
        route(
            &["/cgi-bin/wfs", "request=getfeature", "resulttype=hits"],
            "text/xml",
            br#"<wfs:FeatureCollection xmlns:wfs="http://www.opengis.net/wfs/2.0" numberMatched="42" numberReturned="0"/>"#.to_vec(),
        ),
        route(&["/cgi-bin/wfs", "request=getfeature", "startindex=5"], "text/xml", empty_fc()),
        route(&["/cgi-bin/wfs", "request=getfeature"], "text/xml", gml_page_1),
        route(
            &["/cgi-bin/wfs", "request=describefeaturetype"],
            "text/xml",
            fixture("wfs200_mapserver_describe.xml"),
        ),
    ])
    .await;

    let dwd_wfs = WebFeatureService::connect(
        &format!("{}/geoserver/wfs", server.base),
        WfsVersion::V2_0_0,
    )
    .await
    .unwrap();
    let resp = dwd_wfs
        .get_feature(&GetFeature::new(["dwd:RBSN_T2m"]).count(3))
        .await
        .unwrap();
    assert!(matches!(resp, FeatureResponse::GeoJson(_)));
    assert_eq!(resp.len(), Some(3));

    let wfs =
        WebFeatureService::connect(&format!("{}/cgi-bin/wfs", server.base), WfsVersion::V2_0_0)
            .await
            .unwrap();
    let gml = wfs
        .get_feature(
            &GetFeature::new(["ms:cities"])
                .output_format(OutputFormat::Gml)
                .filter(Filter::greater_than("POPULATION", 1_000_000)),
        )
        .await
        .unwrap();
    assert!(matches!(gml, FeatureResponse::Gml(_)));
    assert_eq!(gml.to_geojson().unwrap().features.len(), 5);

    assert_eq!(
        wfs.hits(&GetFeature::new(["ms:cities"])).await.unwrap(),
        Some(42)
    );

    let mut pager = wfs.pages(
        GetFeature::new(["ms:cities"]).output_format(OutputFormat::Gml),
        5,
    );
    let mut total = 0;
    while let Some(page) = pager.next_page().await.unwrap() {
        total += page.len().unwrap();
    }
    assert_eq!(total, 5);

    let schema = wfs.describe_feature_type("ms:cities").await.unwrap();
    assert_eq!(schema.geometry_property().unwrap().name, "geom");

    let lines: Vec<String> = server.requests().into_iter().map(|r| r.line).collect();
    assert!(lines.iter().any(|l| l.contains("FILTER=<fes:Filter")));
    assert!(
        lines
            .iter()
            .any(|l| l.contains("STARTINDEX=5") && l.contains("COUNT=5"))
    );
}

fn empty_fc() -> Vec<u8> {
    br#"<wfs:FeatureCollection xmlns:wfs="http://www.opengis.net/wfs/2.0" numberMatched="5" numberReturned="0"></wfs:FeatureCollection>"#.to_vec()
}

#[tokio::test]
async fn csw_post_get_records_and_by_id() {
    let pycsw = ["https://demo.pycsw.org"];
    let server = serve(vec![
        route(
            &["get ", "request=getcapabilities"],
            "application/xml",
            rehost("csw_pycsw.xml", &pycsw),
        ),
        route(
            &["post ", "<csw:getrecords"],
            "application/xml",
            fixture("csw_getrecords.xml"),
        ),
        route(
            &["get ", "request=getrecordbyid"],
            "application/xml",
            fixture("csw_getrecordbyid.xml"),
        ),
        route(
            &["get ", "request=getrecords"],
            "application/xml",
            fixture("csw_getrecords10.xml"),
        ),
    ])
    .await;
    let csw = CatalogueService::connect(&format!("{}/cite/csw", server.base))
        .await
        .unwrap();
    let res = csw
        .get_records(
            &GetRecords::new()
                .filter(Filter::like("csw:AnyText", "*lorem*"))
                .element_set(ElementSet::Full)
                .max_records(3),
        )
        .await
        .unwrap();
    assert_eq!(res.records.len(), 3);
    let post = server
        .requests()
        .into_iter()
        .find(|r| r.line.starts_with("POST"))
        .unwrap();
    assert!(post.body.contains("<ogc:PropertyIsLike"));
    assert!(
        post.headers
            .to_ascii_lowercase()
            .contains("content-type: application/xml")
    );

    let one = csw
        .get_record_by_id(
            &["urn:uuid:19887a8a-f6b0-4a63-ae56-7fba0e17801f"],
            ElementSet::Full,
            None,
        )
        .await
        .unwrap();
    assert_eq!(one.records[0].title.as_deref(), Some("Lorem ipsum"));

    let get = csw
        .clone()
        .with_method(ogc_client::csw::HttpMethod::Get)
        .get_records(&GetRecords::new().cql("dc:title like '%a%'"))
        .await
        .unwrap();
    assert_eq!(get.records.len(), 10);
}

#[tokio::test]
async fn ogcapi_features_records_tiles() {
    let pygeo = ["https://demo.pygeoapi.io"];
    let page2 = String::from_utf8(rehost("oapi_lakes_items.json", &pygeo))
        .unwrap()
        .replace("\"rel\":\"next\"", "\"rel\":\"last-page-no-next\"");
    let routes: Vec<Route> = vec![
        route(
            &["get /master/collections/lakes/items/1 "],
            "application/geo+json",
            feature_json(),
        ),
        route(
            &["/collections/lakes/items", "offset=3"],
            "application/geo+json",
            page2.into_bytes(),
        ),
        route(
            &["/collections/lakes/items"],
            "application/geo+json",
            rehost("oapi_lakes_items.json", &pygeo),
        ),
        route(
            &["/collections/dutch-metadata/items"],
            "application/geo+json",
            rehost("oapi_records_items.json", &pygeo),
        ),
        route(
            &["/collections/lakes/tiles/webmercatorquad/3/2/4"],
            "application/vnd.mapbox-vector-tile",
            b"MVT".to_vec(),
        ),
        route(
            &["/collections/lakes/tiles/webmercatorquad?f=json"],
            "application/json",
            rehost("oapi_lakes_tileset.json", &pygeo),
        ),
        route(
            &["/collections/lakes/tiles "],
            "application/json",
            rehost("oapi_lakes_tiles.json", &pygeo),
        ),
        route(
            &["/tilematrixsets/webmercatorquad"],
            "application/json",
            fixture("oapi_tms_webmercator.json"),
        ),
        route(
            &["get /master/collections "],
            "application/json",
            rehost("oapi_collections.json", &pygeo),
        ),
        route(
            &["get /master "],
            "application/json",
            rehost("oapi_landing.json", &pygeo),
        ),
    ];
    let server = serve(routes).await;
    let root = format!("{}/master", server.base);

    let api = Features::new(&root).unwrap();
    let lp = api.landing_page().await.unwrap();
    assert!(lp.title.is_some());
    let all = api
        .all_items("lakes", &ItemsQuery::new().limit(3), None)
        .await
        .unwrap();
    assert_eq!(all.len(), 6); // two pages of three
    let one = api.item("lakes", "1").await.unwrap();
    assert_eq!(one.property("name").and_then(|v| v.as_str()), Some("Test"));
    let accept = server.requests()[1].headers.to_ascii_lowercase();
    assert!(accept.contains("accept: application/geo+json"), "{accept}");

    let rec = Records::new(&root).unwrap();
    let cats = rec.catalogues().await.unwrap();
    assert!(cats.iter().any(|c| c.id == "dutch-metadata"));
    let page = rec
        .records("dutch-metadata", &ItemsQuery::new().q("kaart").limit(2))
        .await
        .unwrap();
    assert_eq!(
        RecordView(&page.features.features[0]).title(),
        Some("Kaartboeck 1635")
    );
    assert!(server.requests().iter().any(|r| r.line.contains("q=kaart")));

    let tiles = Tiles::new(&root).unwrap();
    let list = tiles.collection_tilesets("lakes").await.unwrap();
    let ts = tiles.tileset_from_summary(&list.tilesets[0]).await.unwrap();
    let tms = tiles
        .tile_matrix_set(&ts)
        .await
        .unwrap()
        .to_tile_matrix_set();
    let idx = tms.tile_at("3", 261_845.0, 6_250_566.0).unwrap();
    let tile = tiles.get_tile(&ts, "3", idx.row, idx.col).await.unwrap();
    assert_eq!(&tile.body[..], b"MVT");
}

fn feature_json() -> Vec<u8> {
    br#"{"type":"Feature","id":1,"geometry":{"type":"Point","coordinates":[1,2]},"properties":{"name":"Test"}}"#.to_vec()
}

#[cfg(feature = "blocking")]
#[test]
fn blocking_wrappers() {
    // Run the mock server on its own runtime thread.
    let rt = tokio::runtime::Runtime::new().unwrap();
    let pygeo = ["https://demo.pygeoapi.io"];
    let server = rt.block_on(serve(vec![
        route(
            &["/collections/lakes/items"],
            "application/geo+json",
            rehost("oapi_lakes_items.json", &pygeo),
        ),
        route(
            &["request=getcapabilities", "service=wms"],
            "text/xml",
            fixture("wms111_terrestris.xml"),
        ),
    ]));
    let base = server.base.clone();
    std::thread::spawn(move || {
        let api = ogc_client::blocking::Features::new(&format!("{base}/master")).unwrap();
        let page = api.items("lakes", &ItemsQuery::new().limit(3)).unwrap();
        assert_eq!(page.len(), 3);
        let wms = ogc_client::blocking::WebMapService::connect(
            &format!("{base}/osm/service"),
            WmsVersion::V1_1_1,
        )
        .unwrap();
        assert!(wms.capabilities().layer("OSM-WMS").is_some());
    })
    .join()
    .unwrap();
    drop(rt);
}
