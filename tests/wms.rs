mod common;

use common::fixture;
use ogc_client::Client;
use ogc_client::wms::{
    AxisPolicy, GetFeatureInfo, GetMap, WebMapService, WmsCapabilities, WmsVersion,
};

#[test]
fn usgs_130_capabilities() {
    let caps = WmsCapabilities::parse(&fixture("wms130_usgs.xml")).unwrap();
    assert_eq!(caps.version, WmsVersion::V1_3_0);
    assert_eq!(caps.service.name.as_deref(), Some("WMS"));
    assert!(caps.service.title.is_some());
    let gm = caps.operation("GetMap").unwrap();
    assert!(gm.formats.iter().any(|f| f == "image/png"));
    assert!(
        gm.get_url()
            .unwrap()
            .starts_with("https://basemap.nationalmap.gov:443/")
    );
    assert!(caps.operation("GetFeatureInfo").is_some());

    let layer = caps.layer("0").unwrap();
    // CRS inherited from the root layer.
    assert!(layer.supports_crs("EPSG:4326"));
    assert!(layer.supports_crs("urn:ogc:def:crs:EPSG::3857"));
    let geo = layer.geographic_bbox.as_ref().unwrap();
    assert!(geo.minx < -179.0 && geo.maxx > 179.0);
    // WMS 1.3.0 EPSG:4326 bbox is lat/lon in the document; bbox_for normalizes.
    let (minx, miny, maxx, maxy) = layer.bbox_for("EPSG:4326", caps.version).unwrap();
    assert!(minx < -179.0 && maxx > 179.0, "{minx} {maxx}");
    assert!(miny >= -90.0 && maxy <= 90.0);
    assert_eq!(
        layer.styles.first().map(|s| s.name.as_str()),
        Some("default")
    );
}

#[test]
fn usgs_111_capabilities() {
    let caps = WmsCapabilities::parse(&fixture("wms111_usgs.xml")).unwrap();
    assert_eq!(caps.version, WmsVersion::V1_1_1);
    let layer = caps.layer("0").unwrap();
    assert!(layer.supports_crs("EPSG:4326"));
    let (minx, _, maxx, _) = layer.bbox_for("EPSG:4326", caps.version).unwrap();
    assert!(minx < -179.0 && maxx > 179.0);
}

#[test]
fn terrestris_111_contact_styles_and_inheritance() {
    let caps = WmsCapabilities::parse(&fixture("wms111_terrestris.xml")).unwrap();
    assert_eq!(caps.version, WmsVersion::V1_1_1);
    assert_eq!(caps.service.title.as_deref(), Some("OpenStreetMap WMS"));
    let c = &caps.provider.contact;
    assert_eq!(c.city.as_deref(), Some("Bonn"));
    assert_eq!(c.country.as_deref(), Some("Germany"));
    assert_eq!(c.email.as_deref(), Some("info@terrestris.de"));
    assert_eq!(
        caps.provider.name.as_deref(),
        Some("terrestris GmbH und Co. KG")
    );
    assert!(
        caps.exception_formats
            .contains(&"application/vnd.ogc.se_xml".to_string())
    );

    let root = &caps.layers[0];
    assert!(root.name.is_none());
    let osm = caps.layer("OSM-WMS").unwrap();
    assert!(osm.queryable);
    // 24 SRS declared on the parent, inherited by the child.
    assert_eq!(osm.crs.len(), root.crs.len());
    assert!(osm.crs.len() >= 20);
    assert!(osm.supports_crs("EPSG:25832"));
    let legend = &osm.styles[0].legend_urls[0];
    assert_eq!(legend.width, Some(155));
    assert!(legend.url.contains("layer=OSM-WMS"));
    // LatLonBoundingBox overridden by the child.
    assert_eq!(osm.geographic_bbox.as_ref().unwrap().maxy, 88.0);
    assert!(caps.named_layers().len() >= 4);
}

#[test]
fn gibs_dimensions_and_scale() {
    let caps = WmsCapabilities::parse(&fixture("wms130_gibs.xml")).unwrap();
    assert!(caps.named_layers().len() > 1000);
    let with_time = caps
        .named_layers()
        .into_iter()
        .find(|l| l.dimension("time").is_some())
        .unwrap();
    let t = with_time.dimension("time").unwrap();
    assert_eq!(t.units.as_deref(), Some("ISO8601"));
    assert!(t.default.is_some());
    assert_eq!(t.nearest_value, Some(false));
    assert!(t.value_list().iter().all(|v| v.contains('/')));
}

#[test]
fn mapserver_layers() {
    let caps = WmsCapabilities::parse(&fixture("wms130_mapserver.xml")).unwrap();
    let names: Vec<_> = caps
        .named_layers()
        .into_iter()
        .filter_map(|l| l.name.clone())
        .collect();
    for n in ["bluemarble", "continents", "country_bounds", "cities"] {
        assert!(names.contains(&n.to_string()), "{names:?}");
    }
}

fn usgs_service() -> WebMapService {
    WebMapService::from_capabilities(
        Client::new(),
        "https://basemap.nationalmap.gov/arcgis/services/USGSTopo/MapServer/WMSServer?request=GetCapabilities&service=WMS",
        &fixture("wms130_usgs.xml"),
    )
    .unwrap()
}

#[test]
fn getmap_url_axis_order_130_vs_111() {
    let wms = usgs_service();
    let req = GetMap::new(["0"], (-78.0, 38.0, -77.0, 39.0), "EPSG:4326", 256, 256)
        .format("image/png")
        .transparent(true);
    let url = wms.get_map_url(&req).unwrap();
    let q: Vec<(String, String)> = url.query_pairs().into_owned().collect();
    let get = |k: &str| q.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    assert_eq!(get("VERSION"), Some("1.3.0"));
    assert_eq!(get("CRS"), Some("EPSG:4326"));
    assert_eq!(get("BBOX"), Some("38,-78,39,-77")); // swapped for 1.3.0
    assert_eq!(get("TRANSPARENT"), Some("TRUE"));
    assert_eq!(get("FORMAT"), Some("image/png"));
    assert_eq!(get("STYLES"), Some(""));

    let as_given = wms
        .get_map_url(&req.clone().axis_policy(AxisPolicy::AsGiven))
        .unwrap();
    assert!(as_given.as_str().contains("BBOX=-78%2C38%2C-77%2C39"));

    let crs84 = wms
        .get_map_url(&GetMap::new(
            ["0"],
            (-78.0, 38.0, -77.0, 39.0),
            "CRS:84",
            10,
            10,
        ))
        .unwrap();
    assert!(crs84.as_str().contains("BBOX=-78%2C38%2C-77%2C39"));

    let wms111 = WebMapService::from_capabilities(
        Client::new(),
        "https://example.com/wms",
        &fixture("wms111_usgs.xml"),
    )
    .unwrap();
    let url = wms111.get_map_url(&req).unwrap();
    let s = url.as_str();
    assert!(s.contains("SRS=EPSG%3A4326"), "{s}");
    assert!(s.contains("BBOX=-78%2C38%2C-77%2C39"));
}

#[test]
fn getfeatureinfo_url() {
    let wms = usgs_service();
    let map = GetMap::new(["0"], (-78.0, 38.0, -77.0, 39.0), "EPSG:4326", 256, 256);
    let url = wms
        .get_feature_info_url(
            &GetFeatureInfo::new(map, 128, 64)
                .info_format("text/xml")
                .feature_count(3),
        )
        .unwrap();
    let s = url.as_str();
    assert!(s.contains("REQUEST=GetFeatureInfo"));
    assert!(s.contains("QUERY_LAYERS=0"));
    assert!(s.contains("I=128") && s.contains("J=64"));
    assert!(s.contains("INFO_FORMAT=text%2Fxml"));
    assert!(s.contains("FEATURE_COUNT=3"));
}

#[tokio::test]
async fn unknown_layer_is_rejected_before_network() {
    let wms = usgs_service();
    let err = wms
        .get_map(&GetMap::new(
            ["nope"],
            (0.0, 0.0, 1.0, 1.0),
            "EPSG:4326",
            1,
            1,
        ))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("unknown layer"));
}

#[test]
fn exception_report_becomes_error() {
    let err = WmsCapabilities::parse(&fixture("wms_exception.xml")).unwrap_err();
    match err {
        ogc_client::Error::Service(list) => {
            assert!(!list.is_empty());
            assert!(!list[0].text.is_empty());
        }
        other => panic!("{other:?}"),
    }
}
