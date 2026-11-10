//! Web Feature Service (WFS) 1.1.0 and 2.0.0.
//!
//! ```no_run
//! # async fn run() -> ogc_client::Result<()> {
//! use ogc_client::fes::Filter;
//! use ogc_client::wfs::{GetFeature, WebFeatureService, WfsVersion};
//!
//! let wfs = WebFeatureService::connect("https://maps.dwd.de/geoserver/wfs", WfsVersion::V2_0_0).await?;
//! let resp = wfs
//!     .get_feature(
//!         &GetFeature::new(["dwd:RBSN_T2m"])
//!             .bbox((5.0, 47.0, 15.0, 55.0), Some("EPSG:4326"))
//!             .count(10),
//!     )
//!     .await?;
//! // GeoJSON when the server offers it, GML otherwise; either way:
//! let fc = resp.to_geojson()?;
//! println!("{} features", fc.features.len());
//! # Ok(()) }
//! ```

use serde::{Deserialize, Serialize};
use url::Url;

use crate::crs::{AxisOrder, Crs};
use crate::error::{Error, Result, parse_xml_checked, sniff_exception};
use crate::fes::{FesVersion, Filter};
use crate::gml::GmlFeatureCollection;
use crate::http::{Client, Kvp, build_url, clean_service_url, fmt_num};
use crate::ows::{
    Bbox, BoundingBox, MetadataLink, Operation, ServiceIdentification, ServiceProvider, keywords,
    operations_from_ows, ows_value_map,
};
use crate::xml::Element;

/// WFS protocol version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WfsVersion {
    /// 1.1.0 (`TYPENAME`, `MAXFEATURES`, FES 1.1).
    V1_1_0,
    /// 2.0.0 (`TYPENAMES`, `COUNT`, `STARTINDEX`, FES 2.0).
    V2_0_0,
}

impl WfsVersion {
    /// Version string.
    pub fn as_str(&self) -> &'static str {
        match self {
            WfsVersion::V1_1_0 => "1.1.0",
            WfsVersion::V2_0_0 => "2.0.0",
        }
    }
    /// Filter encoding version used by this WFS version.
    pub fn fes(&self) -> FesVersion {
        match self {
            WfsVersion::V1_1_0 => FesVersion::V1_1,
            WfsVersion::V2_0_0 => FesVersion::V2_0,
        }
    }
}

/// A feature type advertised by a WFS.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeatureType {
    /// Qualified name, e.g. `ms:cities`.
    pub name: String,
    /// Title.
    pub title: Option<String>,
    /// Abstract.
    pub abstract_: Option<String>,
    /// Keywords.
    pub keywords: Vec<String>,
    /// Default CRS (`DefaultCRS` / `DefaultSRS`).
    pub default_crs: Option<String>,
    /// Other CRSs.
    pub other_crs: Vec<String>,
    /// Output formats specific to this feature type.
    pub output_formats: Vec<String>,
    /// WGS 84 bounding box (lon/lat).
    pub wgs84_bbox: Option<BoundingBox>,
    /// Metadata links.
    pub metadata_urls: Vec<MetadataLink>,
}

impl FeatureType {
    /// All CRSs, default first.
    pub fn crs_list(&self) -> Vec<&str> {
        self.default_crs
            .iter()
            .chain(self.other_crs.iter())
            .map(String::as_str)
            .collect()
    }
}

/// Parts of the filter capabilities relevant to clients.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FilterCapabilities {
    /// Spatial operators (`BBOX`, `Intersects`, ...).
    pub spatial_operators: Vec<String>,
    /// Comparison operators (`PropertyIsEqualTo`, `LessThan`, ...).
    pub comparison_operators: Vec<String>,
    /// Conformance constraints (FES 2.0), e.g. `ImplementsMinStandardFilter` → `TRUE`.
    pub conformance: std::collections::BTreeMap<String, Vec<String>>,
}

/// Parsed WFS capabilities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WfsCapabilities {
    /// Version.
    pub version: WfsVersion,
    /// Service identification.
    pub identification: ServiceIdentification,
    /// Service provider.
    pub provider: ServiceProvider,
    /// Operations.
    pub operations: Vec<Operation>,
    /// Service-wide constraints (`ImplementsResultPaging`, `CountDefault`, ...).
    pub constraints: std::collections::BTreeMap<String, Vec<String>>,
    /// Feature types.
    pub feature_types: Vec<FeatureType>,
    /// Filter capabilities.
    pub filter_capabilities: FilterCapabilities,
}

impl WfsCapabilities {
    /// Parses a WFS 1.1.0 or 2.0.0 capabilities document.
    pub fn parse(bytes: &[u8]) -> Result<WfsCapabilities> {
        let root = parse_xml_checked(bytes)?;
        if !root.is("WFS_Capabilities") {
            return Err(Error::Parse(format!(
                "expected WFS_Capabilities, found <{}>",
                root.name
            )));
        }
        let version = match root.attr("version") {
            Some(v) if v.starts_with("1.") => WfsVersion::V1_1_0,
            _ => WfsVersion::V2_0_0,
        };
        let om = root.child("OperationsMetadata");
        let feature_types = root
            .child("FeatureTypeList")
            .map(|l| {
                l.children_named("FeatureType")
                    .map(parse_feature_type)
                    .collect()
            })
            .unwrap_or_default();
        let fc = root.child("Filter_Capabilities");
        let names = |path: &[&str], child: &str| -> Vec<String> {
            fc.and_then(|f| f.path(path))
                .map(|e| {
                    e.children_named(child)
                        .filter_map(|c| c.attr("name").map(str::to_string).or_else(|| c.text_opt()))
                        .collect()
                })
                .unwrap_or_default()
        };
        let filter_capabilities = FilterCapabilities {
            spatial_operators: names(
                &["Spatial_Capabilities", "SpatialOperators"],
                "SpatialOperator",
            ),
            comparison_operators: names(
                &["Scalar_Capabilities", "ComparisonOperators"],
                "ComparisonOperator",
            ),
            conformance: fc
                .and_then(|f| f.child("Conformance"))
                .map(|c| ows_value_map(c, "Constraint"))
                .unwrap_or_default(),
        };
        Ok(WfsCapabilities {
            version,
            identification: root
                .child("ServiceIdentification")
                .map(ServiceIdentification::from_ows)
                .unwrap_or_default(),
            provider: root
                .child("ServiceProvider")
                .map(ServiceProvider::from_ows)
                .unwrap_or_default(),
            operations: om.map(operations_from_ows).unwrap_or_default(),
            constraints: om
                .map(|o| ows_value_map(o, "Constraint"))
                .unwrap_or_default(),
            feature_types,
            filter_capabilities,
        })
    }

    /// Feature type by name (exact, or matching the local part).
    pub fn feature_type(&self, name: &str) -> Option<&FeatureType> {
        self.feature_types
            .iter()
            .find(|f| f.name == name)
            .or_else(|| {
                self.feature_types
                    .iter()
                    .find(|f| f.name.rsplit(':').next() == Some(name))
            })
    }

    /// Operation by name.
    pub fn operation(&self, name: &str) -> Option<&Operation> {
        self.operations
            .iter()
            .find(|o| o.name.eq_ignore_ascii_case(name))
    }

    /// Whether the server declares `ImplementsResultPaging` (WFS 2.0).
    pub fn supports_paging(&self) -> bool {
        self.constraints
            .get("ImplementsResultPaging")
            .map(|v| v.iter().any(|x| x.eq_ignore_ascii_case("true")))
            .unwrap_or(false)
    }

    /// GetFeature output formats for a feature type (type-specific first).
    pub fn output_formats(&self, type_name: Option<&str>) -> Vec<String> {
        let mut out: Vec<String> = type_name
            .and_then(|t| self.feature_type(t))
            .map(|f| f.output_formats.clone())
            .unwrap_or_default();
        if let Some(op) = self.operation("GetFeature") {
            for f in &op.formats {
                if !out.contains(f) {
                    out.push(f.clone());
                }
            }
        }
        out
    }

    /// The GeoJSON output format offered for a type, if any.
    pub fn geojson_format(&self, type_name: Option<&str>) -> Option<String> {
        let formats = self.output_formats(type_name);
        const PREFERRED: [&str; 6] = [
            "application/geo+json",
            "application/json",
            "application/vnd.geo+json",
            "application/json; subtype=geojson",
            "geojson",
            "json",
        ];
        PREFERRED
            .iter()
            .find_map(|p| formats.iter().find(|f| f.eq_ignore_ascii_case(p)).cloned())
    }
}

fn parse_feature_type(e: &Element) -> FeatureType {
    let mut other_crs = e.children_text("OtherCRS");
    other_crs.extend(e.children_text("OtherSRS"));
    FeatureType {
        name: e.child_text("Name").unwrap_or_default(),
        title: e.child_text("Title"),
        abstract_: e.child_text("Abstract"),
        keywords: keywords(e),
        default_crs: e
            .child_text("DefaultCRS")
            .or_else(|| e.child_text("DefaultSRS")),
        other_crs,
        output_formats: e
            .child("OutputFormats")
            .map(|o| o.children_text("Format"))
            .unwrap_or_default(),
        wgs84_bbox: e.child("WGS84BoundingBox").and_then(BoundingBox::from_ows),
        metadata_urls: e
            .children_named("MetadataURL")
            .filter_map(MetadataLink::from_wms)
            .collect(),
    }
}

/// Requested output.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum OutputFormat {
    /// GeoJSON if the server advertises it, GML otherwise (default).
    #[default]
    PreferGeoJson,
    /// The server's default (GML).
    Gml,
    /// An explicit `OUTPUTFORMAT` value.
    Custom(String),
}

/// `RESULTTYPE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ResultType {
    /// Return features.
    #[default]
    Results,
    /// Return only the count.
    Hits,
}

/// A GetFeature request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GetFeature {
    /// Feature type names.
    pub type_names: Vec<String>,
    /// `(minx, miny, maxx, maxy)` in east/north order plus optional CRS.
    pub bbox: Option<(Bbox, Option<String>)>,
    /// Filter (combined with `bbox` if both are set).
    pub filter: Option<Filter>,
    /// Raw filter XML (used instead of `filter`).
    pub filter_xml: Option<String>,
    /// Feature ids (`RESOURCEID` / `FEATUREID`).
    pub feature_ids: Vec<String>,
    /// Properties to return.
    pub property_names: Vec<String>,
    /// Sort keys (`NAME` or `NAME DESC`).
    pub sort_by: Vec<String>,
    /// Maximum number of features (`COUNT` / `MAXFEATURES`).
    pub count: Option<u64>,
    /// Paging offset (`STARTINDEX`).
    pub start_index: Option<u64>,
    /// Output CRS (`SRSNAME`).
    pub srs_name: Option<String>,
    /// Output format.
    pub output_format: OutputFormat,
    /// Result type.
    pub result_type: ResultType,
    /// Stored query id (WFS 2.0) with its parameters in `extra`.
    pub stored_query_id: Option<String>,
    /// Vendor / stored query parameters.
    pub extra: Vec<(String, String)>,
}

impl GetFeature {
    /// A request for the given feature types.
    pub fn new<I: IntoIterator<Item = S>, S: Into<String>>(type_names: I) -> GetFeature {
        GetFeature {
            type_names: type_names.into_iter().map(Into::into).collect(),
            bbox: None,
            filter: None,
            filter_xml: None,
            feature_ids: Vec::new(),
            property_names: Vec::new(),
            sort_by: Vec::new(),
            count: None,
            start_index: None,
            srs_name: None,
            output_format: OutputFormat::PreferGeoJson,
            result_type: ResultType::Results,
            stored_query_id: None,
            extra: Vec::new(),
        }
    }
    /// Spatial bbox (x/y order; swapped automatically for y/x CRSs).
    pub fn bbox(mut self, bbox: (f64, f64, f64, f64), crs: Option<&str>) -> GetFeature {
        self.bbox = Some((bbox, crs.map(str::to_string)));
        self
    }
    /// Attribute/spatial filter.
    pub fn filter(mut self, f: Filter) -> GetFeature {
        self.filter = Some(f);
        self
    }
    /// Raw `<Filter>` XML.
    pub fn filter_xml(mut self, xml: &str) -> GetFeature {
        self.filter_xml = Some(xml.to_string());
        self
    }
    /// Feature ids.
    pub fn feature_ids<I: IntoIterator<Item = S>, S: Into<String>>(mut self, ids: I) -> GetFeature {
        self.feature_ids = ids.into_iter().map(Into::into).collect();
        self
    }
    /// Properties to return.
    pub fn property_names<I: IntoIterator<Item = S>, S: Into<String>>(
        mut self,
        p: I,
    ) -> GetFeature {
        self.property_names = p.into_iter().map(Into::into).collect();
        self
    }
    /// Sort keys.
    pub fn sort_by(mut self, key: &str) -> GetFeature {
        self.sort_by.push(key.to_string());
        self
    }
    /// Page size / maximum features.
    pub fn count(mut self, n: u64) -> GetFeature {
        self.count = Some(n);
        self
    }
    /// Paging offset.
    pub fn start_index(mut self, n: u64) -> GetFeature {
        self.start_index = Some(n);
        self
    }
    /// Output CRS.
    pub fn srs_name(mut self, s: &str) -> GetFeature {
        self.srs_name = Some(s.to_string());
        self
    }
    /// Output format.
    pub fn output_format(mut self, f: OutputFormat) -> GetFeature {
        self.output_format = f;
        self
    }
    /// Result type.
    pub fn result_type(mut self, r: ResultType) -> GetFeature {
        self.result_type = r;
        self
    }
    /// Stored query (WFS 2.0); add parameters with [`GetFeature::param`].
    pub fn stored_query(mut self, id: &str) -> GetFeature {
        self.stored_query_id = Some(id.to_string());
        self
    }
    /// Vendor or stored-query parameter.
    pub fn param(mut self, k: &str, v: &str) -> GetFeature {
        self.extra.push((k.to_string(), v.to_string()));
        self
    }

    fn kvp(&self, version: WfsVersion, output_format: Option<String>) -> Result<Kvp> {
        let mut k = Kvp::new();
        k.push("SERVICE", "WFS")
            .push("VERSION", version.as_str())
            .push("REQUEST", "GetFeature");
        let v2 = version == WfsVersion::V2_0_0;
        if let Some(sq) = &self.stored_query_id {
            if !v2 {
                return Err(Error::Unsupported("stored queries need WFS 2.0".into()));
            }
            k.push("STOREDQUERY_ID", sq.clone());
        } else if self.type_names.is_empty() && self.feature_ids.is_empty() {
            return Err(Error::InvalidRequest("no type names given".into()));
        }
        if !self.type_names.is_empty() {
            k.push(
                if v2 { "TYPENAMES" } else { "TYPENAME" },
                self.type_names.join(","),
            );
        }
        // BBOX, FILTER and RESOURCEID are mutually exclusive in KVP.
        let mut filters: Vec<Filter> = Vec::new();
        if let Some(f) = &self.filter {
            filters.push(f.clone());
        }
        let has_filter = !filters.is_empty() || self.filter_xml.is_some();
        if !self.feature_ids.is_empty() {
            if has_filter || self.bbox.is_some() {
                filters.push(Filter::ids(self.feature_ids.clone()));
            } else {
                k.push(
                    if v2 { "RESOURCEID" } else { "FEATUREID" },
                    self.feature_ids.join(","),
                );
            }
        }
        if let Some((bbox, crs)) = &self.bbox {
            if has_filter {
                if self.filter_xml.is_some() {
                    return Err(Error::InvalidRequest(
                        "cannot combine bbox with a raw filter; put the BBOX in the filter".into(),
                    ));
                }
                filters.push(Filter::bbox(None, *bbox, crs.as_deref()));
            } else {
                k.push("BBOX", kvp_bbox(*bbox, crs.as_deref()));
            }
        }
        if let Some(x) = &self.filter_xml {
            k.push("FILTER", x.clone());
        } else if !filters.is_empty() {
            let f = if filters.len() == 1 {
                filters.remove(0)
            } else {
                Filter::And(filters)
            };
            k.push("FILTER", f.to_xml(version.fes()));
        }
        if !self.property_names.is_empty() {
            k.push("PROPERTYNAME", self.property_names.join(","));
        }
        if !self.sort_by.is_empty() {
            let keys: Vec<String> = self
                .sort_by
                .iter()
                .map(|s| {
                    // WFS 1.1 uses `NAME D`/`NAME A`, 2.0 uses `NAME DESC`/`NAME ASC`.
                    let mut parts = s.split_whitespace();
                    let name = parts.next().unwrap_or_default();
                    match parts.next().map(|d| d.to_ascii_uppercase()) {
                        Some(d) if d.starts_with('D') => {
                            format!("{name} {}", if v2 { "DESC" } else { "D" })
                        }
                        Some(_) => format!("{name} {}", if v2 { "ASC" } else { "A" }),
                        None => name.to_string(),
                    }
                })
                .collect();
            k.push("SORTBY", keys.join(","));
        }
        k.opt(
            if v2 { "COUNT" } else { "MAXFEATURES" },
            self.count.map(|n| n.to_string()),
        )
        .opt("STARTINDEX", self.start_index.map(|n| n.to_string()))
        .opt("SRSNAME", self.srs_name.clone())
        .opt("OUTPUTFORMAT", output_format);
        if self.result_type == ResultType::Hits {
            k.push("RESULTTYPE", "hits");
        }
        k.extend(&self.extra);
        Ok(k)
    }
}

/// Formats a KVP `BBOX` value. With a CRS the URN form is sent and axes are
/// swapped for y/x CRSs, so the meaning is unambiguous on every server.
fn kvp_bbox(bbox: (f64, f64, f64, f64), crs: Option<&str>) -> String {
    let (a, b, c, d) = bbox;
    match crs.and_then(Crs::parse) {
        Some(crs) => {
            let v = if crs.authority_axis_order() == AxisOrder::YX {
                [b, a, d, c]
            } else {
                [a, b, c, d]
            };
            format!(
                "{},{}",
                v.iter().map(|x| fmt_num(*x)).collect::<Vec<_>>().join(","),
                crs.to_urn()
            )
        }
        None => [a, b, c, d]
            .iter()
            .map(|x| fmt_num(*x))
            .collect::<Vec<_>>()
            .join(","),
    }
}

/// A GetFeature response.
#[derive(Debug, Clone)]
pub enum FeatureResponse {
    /// GeoJSON output.
    GeoJson(geojson::FeatureCollection),
    /// GML output.
    Gml(GmlFeatureCollection),
    /// Result of `RESULTTYPE=hits`.
    Hits {
        /// `numberMatched` / `numberOfFeatures`.
        number_matched: Option<u64>,
    },
    /// Anything else (CSV, KML, shapefile ZIP ...).
    Other {
        /// MIME type.
        content_type: Option<String>,
        /// Body.
        body: bytes::Bytes,
    },
}

impl FeatureResponse {
    /// Converts GeoJSON or GML output to a GeoJSON feature collection.
    pub fn to_geojson(&self) -> Result<geojson::FeatureCollection> {
        match self {
            FeatureResponse::GeoJson(fc) => Ok(fc.clone()),
            FeatureResponse::Gml(g) => Ok(g.to_geojson()),
            _ => Err(Error::Unsupported("response is not GeoJSON or GML".into())),
        }
    }

    /// Number of features in this response (`None` for hits/other).
    pub fn len(&self) -> Option<usize> {
        match self {
            FeatureResponse::GeoJson(fc) => Some(fc.features.len()),
            FeatureResponse::Gml(g) => Some(g.features.len()),
            _ => None,
        }
    }

    /// Whether the response contains no features.
    pub fn is_empty(&self) -> bool {
        self.len() == Some(0)
    }

    /// Total number of matching features, if the server reported it.
    pub fn number_matched(&self) -> Option<u64> {
        match self {
            FeatureResponse::GeoJson(fc) => fc
                .foreign_members
                .as_ref()
                .and_then(|m| m.get("numberMatched").or_else(|| m.get("totalFeatures")))
                .and_then(|v| v.as_u64()),
            FeatureResponse::Gml(g) => g.number_matched,
            FeatureResponse::Hits { number_matched } => *number_matched,
            FeatureResponse::Other { .. } => None,
        }
    }

    /// Parses a response body by sniffing its content.
    pub fn parse(content_type: Option<&str>, body: bytes::Bytes) -> Result<FeatureResponse> {
        let trimmed = body
            .iter()
            .position(|b| !b.is_ascii_whitespace())
            .map(|i| &body[i..])
            .unwrap_or(&[]);
        let ct = content_type.unwrap_or_default().to_ascii_lowercase();
        if trimmed.starts_with(b"{") || ct.contains("json") {
            let v: serde_json::Value = serde_json::from_slice(&body)?;
            if let Some(err) = crate::ogcapi::api_error_value(&v) {
                return Err(err);
            }
            return Ok(FeatureResponse::GeoJson(serde_json::from_value(v)?));
        }
        if trimmed.starts_with(b"<") {
            if let Some(err) = sniff_exception(&body) {
                return Err(err);
            }
            let root = parse_xml_checked(&body)?;
            if root.name.ends_with("FeatureCollection") {
                let fc = GmlFeatureCollection::from_element(&root)?;
                let is_hits = fc.features.is_empty()
                    && root
                        .attr("numberReturned")
                        .map(|v| v == "0")
                        .unwrap_or(false)
                    && root.attr("numberMatched").is_some();
                if is_hits && root.children.iter().all(|c| !c.name.contains("ember")) {
                    return Ok(FeatureResponse::Hits {
                        number_matched: fc.number_matched,
                    });
                }
                return Ok(FeatureResponse::Gml(fc));
            }
        }
        Ok(FeatureResponse::Other {
            content_type: content_type.map(str::to_string),
            body,
        })
    }
}

/// A property of a feature type schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemaProperty {
    /// Property name.
    pub name: String,
    /// XSD type (e.g. `string`, `gml:PointPropertyType`).
    pub type_: Option<String>,
    /// `minOccurs`.
    pub min_occurs: Option<u32>,
    /// `maxOccurs` (`None` for unbounded or absent).
    pub max_occurs: Option<String>,
    /// `nillable`.
    pub nillable: bool,
}

impl SchemaProperty {
    /// Whether this is a GML geometry property.
    pub fn is_geometry(&self) -> bool {
        self.type_
            .as_deref()
            .map(|t| {
                let t = t.rsplit(':').next().unwrap_or(t);
                t.ends_with("PropertyType")
                    && [
                        "Geometry",
                        "Point",
                        "LineString",
                        "Polygon",
                        "Surface",
                        "Curve",
                        "MultiPoint",
                        "MultiLineString",
                        "MultiPolygon",
                        "MultiSurface",
                        "MultiCurve",
                        "MultiGeometry",
                    ]
                    .iter()
                    .any(|g| t.starts_with(g))
            })
            .unwrap_or(false)
    }
}

/// A feature type schema from DescribeFeatureType (OWSLib's `get_schema`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeatureSchema {
    /// Element name of the feature type.
    pub name: String,
    /// Target namespace.
    pub target_namespace: Option<String>,
    /// Properties in order.
    pub properties: Vec<SchemaProperty>,
}

impl FeatureSchema {
    /// The first geometry property.
    pub fn geometry_property(&self) -> Option<&SchemaProperty> {
        self.properties.iter().find(|p| p.is_geometry())
    }

    /// Parses an XSD returned by DescribeFeatureType; returns one schema per
    /// top-level element.
    pub fn parse_all(bytes: &[u8]) -> Result<Vec<FeatureSchema>> {
        let root = parse_xml_checked(bytes)?;
        if !root.is("schema") {
            return Err(Error::Parse(format!(
                "expected xsd:schema, found <{}>",
                root.name
            )));
        }
        let tns = root.attr("targetNamespace").map(str::to_string);
        let local = |s: &str| s.rsplit(':').next().unwrap_or(s).to_string();
        let mut out = Vec::new();
        for el in root.children_named("element") {
            let Some(name) = el.attr("name") else {
                continue;
            };
            // Find the complex type: inline or by @type reference.
            let ct = el.child("complexType").or_else(|| {
                let t = local(el.attr("type")?);
                root.children_named("complexType")
                    .find(|c| c.attr("name") == Some(t.as_str()))
            });
            let mut properties = Vec::new();
            if let Some(ct) = ct {
                for seq in ct.descendants().filter(|d| d.is("sequence")) {
                    for p in seq.children_named("element") {
                        let pname = p.attr("name").or_else(|| p.attr("ref"));
                        let Some(pname) = pname else { continue };
                        let type_ = p.attr("type").map(str::to_string).or_else(|| {
                            p.find("restriction")
                                .and_then(|r| r.attr("base"))
                                .map(str::to_string)
                        });
                        properties.push(SchemaProperty {
                            name: local(pname),
                            type_,
                            min_occurs: p.attr_u32("minOccurs"),
                            max_occurs: p.attr("maxOccurs").map(str::to_string),
                            nillable: p.attr_bool("nillable").unwrap_or(false),
                        });
                    }
                }
            }
            out.push(FeatureSchema {
                name: name.to_string(),
                target_namespace: tns.clone(),
                properties,
            });
        }
        Ok(out)
    }
}

/// A connected WFS.
#[derive(Debug, Clone)]
pub struct WebFeatureService {
    url: Url,
    client: Client,
    capabilities: WfsCapabilities,
}

impl WebFeatureService {
    /// Fetches capabilities with a default client.
    pub async fn connect(url: &str, version: WfsVersion) -> Result<WebFeatureService> {
        Self::connect_with(Client::new(), url, version).await
    }

    /// Fetches capabilities with the given client.
    pub async fn connect_with(
        client: Client,
        url: &str,
        version: WfsVersion,
    ) -> Result<WebFeatureService> {
        let caps_url = Self::capabilities_url(url, version)?;
        let resp = client
            .get(&caps_url, Some("application/xml, text/xml"))
            .await?;
        Ok(WebFeatureService {
            url: clean_service_url(url)?,
            client,
            capabilities: WfsCapabilities::parse(&resp.body)?,
        })
    }

    /// Builds a service from a downloaded capabilities document.
    pub fn from_capabilities(client: Client, url: &str, xml: &[u8]) -> Result<WebFeatureService> {
        Ok(WebFeatureService {
            url: clean_service_url(url)?,
            client,
            capabilities: WfsCapabilities::parse(xml)?,
        })
    }

    /// The GetCapabilities URL.
    pub fn capabilities_url(url: &str, version: WfsVersion) -> Result<Url> {
        let mut k = Kvp::new();
        k.push("SERVICE", "WFS")
            .push("REQUEST", "GetCapabilities")
            .push(
                if version == WfsVersion::V2_0_0 {
                    "ACCEPTVERSIONS"
                } else {
                    "VERSION"
                },
                version.as_str(),
            );
        build_url(clean_service_url(url)?.as_str(), &k.0)
    }

    /// Parsed capabilities.
    pub fn capabilities(&self) -> &WfsCapabilities {
        &self.capabilities
    }

    /// Negotiated version.
    pub fn version(&self) -> WfsVersion {
        self.capabilities.version
    }

    /// The HTTP client.
    pub fn client(&self) -> &Client {
        &self.client
    }

    fn endpoint(&self, op: &str) -> String {
        self.capabilities
            .operation(op)
            .and_then(|o| o.get_url())
            .map(str::to_string)
            .unwrap_or_else(|| self.url.to_string())
    }

    fn resolve_format(&self, req: &GetFeature) -> Option<String> {
        match &req.output_format {
            OutputFormat::Gml => None,
            OutputFormat::Custom(f) => Some(f.clone()),
            OutputFormat::PreferGeoJson => {
                if req.result_type == ResultType::Hits {
                    return None;
                }
                self.capabilities
                    .geojson_format(req.type_names.first().map(String::as_str))
            }
        }
    }

    /// The GetFeature URL for a request (no network access).
    pub fn get_feature_url(&self, req: &GetFeature) -> Result<Url> {
        for t in &req.type_names {
            if self.capabilities.feature_type(t).is_none() {
                return Err(Error::InvalidRequest(format!("unknown feature type: {t}")));
            }
        }
        let k = req.kvp(self.version(), self.resolve_format(req))?;
        build_url(&self.endpoint("GetFeature"), &k.0)
    }

    /// Runs GetFeature.
    pub async fn get_feature(&self, req: &GetFeature) -> Result<FeatureResponse> {
        let url = self.get_feature_url(req)?;
        let resp = self.client.get(&url, None).await?;
        FeatureResponse::parse(resp.content_type.as_deref(), resp.body)
    }

    /// Counts matching features with `RESULTTYPE=hits`.
    pub async fn hits(&self, req: &GetFeature) -> Result<Option<u64>> {
        let req = req.clone().result_type(ResultType::Hits);
        Ok(self.get_feature(&req).await?.number_matched())
    }

    /// Iterates over pages of `page_size` features using `STARTINDEX`.
    pub fn pages(&self, req: GetFeature, page_size: u64) -> FeaturePager<'_> {
        FeaturePager {
            wfs: self,
            start: req.start_index.unwrap_or(0),
            req,
            page_size,
            done: false,
        }
    }

    /// The DescribeFeatureType URL.
    pub fn describe_feature_type_url(&self, type_names: &[&str]) -> Result<Url> {
        let v2 = self.version() == WfsVersion::V2_0_0;
        let mut k = Kvp::new();
        k.push("SERVICE", "WFS")
            .push("VERSION", self.version().as_str())
            .push("REQUEST", "DescribeFeatureType")
            .push(
                if v2 { "TYPENAMES" } else { "TYPENAME" },
                type_names.join(","),
            );
        build_url(&self.endpoint("DescribeFeatureType"), &k.0)
    }

    /// Fetches and parses the schema of one feature type.
    pub async fn describe_feature_type(&self, type_name: &str) -> Result<FeatureSchema> {
        let url = self.describe_feature_type_url(&[type_name])?;
        let resp = self.client.get(&url, None).await?;
        let local = type_name.rsplit(':').next().unwrap_or(type_name);
        FeatureSchema::parse_all(&resp.body)?
            .into_iter()
            .find(|s| s.name == local || s.name == type_name)
            .ok_or_else(|| Error::Parse(format!("schema does not describe {type_name}")))
    }
}

/// Pager returned by [`WebFeatureService::pages`].
pub struct FeaturePager<'a> {
    wfs: &'a WebFeatureService,
    req: GetFeature,
    start: u64,
    page_size: u64,
    done: bool,
}

impl FeaturePager<'_> {
    /// Fetches the next page, or `None` when exhausted.
    pub async fn next_page(&mut self) -> Result<Option<FeatureResponse>> {
        if self.done {
            return Ok(None);
        }
        let req = self
            .req
            .clone()
            .start_index(self.start)
            .count(self.page_size);
        let page = self.wfs.get_feature(&req).await?;
        let n = page.len().unwrap_or(0) as u64;
        self.start += n;
        let total_reached = page
            .number_matched()
            .map(|m| self.start >= m)
            .unwrap_or(false);
        if n < self.page_size || total_reached {
            self.done = true;
        }
        if n == 0 {
            return Ok(None);
        }
        Ok(Some(page))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bbox_param() {
        assert_eq!(
            kvp_bbox((1.0, 2.0, 3.0, 4.0), Some("EPSG:4326")),
            "2,1,4,3,urn:ogc:def:crs:EPSG::4326"
        );
        assert_eq!(
            kvp_bbox((1.0, 2.0, 3.0, 4.0), Some("EPSG:3857")),
            "1,2,3,4,urn:ogc:def:crs:EPSG::3857"
        );
        assert_eq!(kvp_bbox((1.0, 2.0, 3.0, 4.0), None), "1,2,3,4");
    }

    #[test]
    fn kvp_combines_bbox_and_filter() {
        let req = GetFeature::new(["a:b"])
            .bbox((1.0, 2.0, 3.0, 4.0), Some("EPSG:3857"))
            .filter(Filter::equal_to("x", 1));
        let k = req.kvp(WfsVersion::V2_0_0, None).unwrap();
        assert!(!k.0.iter().any(|(n, _)| n == "BBOX"));
        let f = &k.0.iter().find(|(n, _)| n == "FILTER").unwrap().1;
        assert!(f.contains("<fes:And>") && f.contains("<fes:BBOX>"));
        let k = GetFeature::new(["a:b"])
            .sort_by("NAME desc")
            .count(5)
            .kvp(WfsVersion::V1_1_0, None)
            .unwrap();
        assert!(k.0.contains(&("SORTBY".into(), "NAME D".into())));
        assert!(k.0.contains(&("MAXFEATURES".into(), "5".into())));
        assert!(k.0.contains(&("TYPENAME".into(), "a:b".into())));
    }
}
