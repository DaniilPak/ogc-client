mod common;

use common::fixture;
use geo_types::Geometry;
use ogc_client::Client;
use ogc_client::fes::Filter;
use ogc_client::gml::GmlFeatureCollection;
use ogc_client::wfs::{
    FeatureResponse, FeatureSchema, GetFeature, OutputFormat, ResultType, WebFeatureService,
    WfsCapabilities, WfsVersion,
};

#[test]
fn mapserver_200_capabilities() {
    let caps = WfsCapabilities::parse(&fixture("wfs200_mapserver.xml")).unwrap();
    assert_eq!(caps.version, WfsVersion::V2_0_0);
    assert_eq!(caps.feature_types.len(), 2);
    let cities = caps.feature_type("ms:cities").unwrap();
    assert!(cities.default_crs.as_deref().unwrap().contains("4326"));
    assert!(!cities.other_crs.is_empty());
    assert!(cities.wgs84_bbox.is_some());
    // local-name lookup
    assert!(caps.feature_type("cities").is_some());
    assert!(caps.operation("GetFeature").is_some());
    assert!(caps.operation("DescribeFeatureType").is_some());
    assert!(caps.supports_paging());
    // MapServer advertises GeoJSON via the subtype form.
    assert_eq!(
        caps.geojson_format(Some("ms:cities")).as_deref(),
        Some("application/json; subtype=geojson")
    );
    assert!(!caps.filter_capabilities.spatial_operators.is_empty());
}

#[test]
fn mapserver_110_capabilities_offers_geojson_per_type() {
    let caps = WfsCapabilities::parse(&fixture("wfs110_mapserver.xml")).unwrap();
    assert_eq!(caps.version, WfsVersion::V1_1_0);
    let ft = caps.feature_type("continents").unwrap();
    assert_eq!(
        ft.default_crs.as_deref(),
        Some("urn:ogc:def:crs:EPSG::4326")
    );
    assert_eq!(ft.other_crs.len(), 3);
    assert_eq!(
        caps.geojson_format(Some("continents")).as_deref(),
        Some("application/json; subtype=geojson")
    );
    assert!(!ft.metadata_urls.is_empty());
}

#[test]
fn dwd_geoserver_capabilities() {
    let caps = WfsCapabilities::parse(&fixture("wfs200_dwd.xml")).unwrap();
    assert!(caps.feature_types.len() > 50);
    let ft = caps.feature_type("dwd:RBSN_T2m").unwrap();
    assert_eq!(
        ft.default_crs.as_deref(),
        Some("urn:ogc:def:crs:EPSG::4258")
    );
    assert!(ft.keywords.contains(&"SYNOP".to_string()));
    // GeoServer offers several formats; we pick real GeoJSON, not JSONP.
    assert_eq!(
        caps.geojson_format(None).as_deref(),
        Some("application/json")
    );
    assert!(caps.supports_paging());
    assert_eq!(
        caps.identification.title.as_deref(),
        Some("DWD GeoServer WFS")
    );
}

#[test]
fn pdok_inspire_capabilities() {
    let caps = WfsCapabilities::parse(&fixture("wfs200_pdok_inspire_cp.xml")).unwrap();
    assert_eq!(caps.feature_types.len(), 1);
    let ft = &caps.feature_types[0];
    assert_eq!(ft.name, "cp:cadastralparcel");
    assert_eq!(
        ft.default_crs.as_deref(),
        Some("urn:ogc:def:crs:EPSG::4258")
    );
    assert!(caps.geojson_format(Some("cp:cadastralparcel")).is_some());
    assert!(caps.provider.name.is_some());
}

#[test]
fn gml32_feature_collection_axis_order_from_urn() {
    let fc = GmlFeatureCollection::parse(&fixture("wfs200_mapserver_getfeature.xml")).unwrap();
    assert_eq!(fc.features.len(), 5);
    assert_eq!(fc.number_returned, Some(5));
    assert_eq!(fc.number_matched, None); // "unknown"
    assert!(fc.next.as_deref().unwrap().contains("STARTINDEX=5"));
    let ba = &fc.features[0];
    assert_eq!(ba.id.as_deref(), Some("cities.8338"));
    assert_eq!(ba.type_name, "cities");
    assert_eq!(ba.geometry_property.as_deref(), Some("geom"));
    assert_eq!(ba.properties["NAME"], "Buenos Aires");
    assert_eq!(ba.properties["POPULATION"], 12116379);
    // urn EPSG:4326 is lat/lon in the document; we return lon/lat.
    match ba.geometry.as_ref().unwrap() {
        Geometry::Point(p) => {
            assert!((p.x() + 58.6725).abs() < 1e-6, "{p:?}");
            assert!((p.y() + 34.5875).abs() < 1e-6);
        }
        g => panic!("{g:?}"),
    }
    let gj = fc.to_geojson();
    assert_eq!(gj.features.len(), 5);
    let json = serde_json::to_value(&gj).unwrap();
    assert_eq!(json["features"][0]["geometry"]["type"], "Point");
    assert_eq!(json["features"][0]["id"], "cities.8338");
}

#[test]
fn gml311_short_epsg_detected_as_latlon() {
    // MapServer WFS 1.1 writes lat/lon even for srsName="EPSG:4326".
    let fc = GmlFeatureCollection::parse(&fixture("wfs110_mapserver_getfeature.xml")).unwrap();
    assert_eq!(fc.features.len(), 5);
    let ba = &fc.features[0];
    assert_eq!(ba.properties["NAME"], "Buenos Aires");
    match ba.geometry.as_ref().unwrap() {
        Geometry::Point(p) => assert!((p.x() + 58.6725).abs() < 1e-6, "{p:?}"),
        g => panic!("{g:?}"),
    }
    // Same answer as the 2.0 document.
    let fc2 = GmlFeatureCollection::parse(&fixture("wfs200_mapserver_getfeature.xml")).unwrap();
    for (a, b) in fc.features.iter().zip(fc2.features.iter()) {
        assert_eq!(a.geometry, b.geometry);
    }
}

#[test]
fn geojson_response() {
    let body = bytes::Bytes::from(fixture("wfs200_dwd_getfeature.json"));
    let resp = FeatureResponse::parse(Some("application/json"), body).unwrap();
    let fc = resp.to_geojson().unwrap();
    assert_eq!(fc.features.len(), 3);
    assert!(fc.features[0].geometry.is_some());
    assert!(resp.number_matched().is_some());
}

#[test]
fn exception_response_is_error() {
    let body = bytes::Bytes::from(fixture("ows_exception.xml"));
    let err = FeatureResponse::parse(Some("text/xml"), body).unwrap_err();
    assert!(matches!(err, ogc_client::Error::Service(_)), "{err:?}");
}

#[test]
fn describe_feature_type_schema() {
    let schemas = FeatureSchema::parse_all(&fixture("wfs200_mapserver_describe.xml")).unwrap();
    assert_eq!(schemas.len(), 1);
    let s = &schemas[0];
    assert_eq!(s.name, "cities");
    assert_eq!(
        s.target_namespace.as_deref(),
        Some("http://mapserver.gis.umn.edu/mapserver")
    );
    let names: Vec<_> = s.properties.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["geom", "POPULATION", "NAME"]);
    assert_eq!(s.geometry_property().unwrap().name, "geom");
    assert_eq!(s.properties[1].type_.as_deref(), Some("integer"));
}

#[test]
fn getfeature_urls() {
    let wfs = WebFeatureService::from_capabilities(
        Client::new(),
        "https://maps.dwd.de/geoserver/wfs",
        &fixture("wfs200_dwd.xml"),
    )
    .unwrap();
    // GeoJSON chosen automatically, bbox in URN form with lat/lon order.
    let url = wfs
        .get_feature_url(
            &GetFeature::new(["dwd:RBSN_T2m"])
                .bbox((5.0, 47.0, 15.0, 55.0), Some("EPSG:4326"))
                .count(10)
                .start_index(20),
        )
        .unwrap();
    let q: Vec<(String, String)> = url.query_pairs().into_owned().collect();
    let get = |k: &str| q.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    assert_eq!(get("TYPENAMES").as_deref(), Some("dwd:RBSN_T2m"));
    assert_eq!(
        get("BBOX").as_deref(),
        Some("47,5,55,15,urn:ogc:def:crs:EPSG::4326")
    );
    assert_eq!(get("COUNT").as_deref(), Some("10"));
    assert_eq!(get("STARTINDEX").as_deref(), Some("20"));
    assert_eq!(get("OUTPUTFORMAT").as_deref(), Some("application/json"));

    // Filter + bbox are merged into one FES 2.0 filter.
    let url = wfs
        .get_feature_url(
            &GetFeature::new(["dwd:RBSN_T2m"])
                .bbox((5.0, 47.0, 15.0, 55.0), Some("EPSG:4326"))
                .filter(Filter::greater_than("TT", 10))
                .output_format(OutputFormat::Gml),
        )
        .unwrap();
    let q: Vec<(String, String)> = url.query_pairs().into_owned().collect();
    let filter = &q.iter().find(|(n, _)| n == "FILTER").unwrap().1;
    assert!(filter.contains("<fes:And>"));
    assert!(filter.contains("<fes:PropertyIsGreaterThan>"));
    assert!(!q.iter().any(|(n, _)| n == "BBOX" || n == "OUTPUTFORMAT"));

    let hits = wfs
        .get_feature_url(&GetFeature::new(["dwd:RBSN_T2m"]).result_type(ResultType::Hits))
        .unwrap();
    assert!(hits.as_str().contains("RESULTTYPE=hits"));
    assert!(!hits.as_str().contains("OUTPUTFORMAT"));

    assert!(wfs.get_feature_url(&GetFeature::new(["dwd:nope"])).is_err());
}

#[test]
fn getfeature_url_wfs110() {
    let wfs = WebFeatureService::from_capabilities(
        Client::new(),
        "https://demo.mapserver.org/cgi-bin/wfs",
        &fixture("wfs110_mapserver.xml"),
    )
    .unwrap();
    let url = wfs
        .get_feature_url(
            &GetFeature::new(["cities"])
                .count(5)
                .property_names(["NAME"])
                .sort_by("NAME DESC"),
        )
        .unwrap();
    let s = url.as_str();
    assert!(s.contains("VERSION=1.1.0"));
    assert!(s.contains("TYPENAME=cities"));
    assert!(s.contains("MAXFEATURES=5"));
    assert!(s.contains("SORTBY=NAME+D"));
    assert!(s.contains("OUTPUTFORMAT=application%2Fjson%3B+subtype%3Dgeojson"));
    let dft = wfs.describe_feature_type_url(&["cities"]).unwrap();
    assert!(dft.as_str().contains("REQUEST=DescribeFeatureType"));
    assert!(dft.as_str().contains("TYPENAME=cities"));
}
