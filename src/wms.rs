//! Web Map Service (WMS) 1.1.1 and 1.3.0.
//!
//! ```no_run
//! # async fn run() -> ogc_client::Result<()> {
//! use ogc_client::wms::{GetMap, WebMapService, WmsVersion};
//!
//! let wms = WebMapService::connect(
//!     "https://basemap.nationalmap.gov/arcgis/services/USGSTopo/MapServer/WMSServer",
//!     WmsVersion::V1_3_0,
//! )
//! .await?;
//! println!("{:?}", wms.capabilities().service.title);
//! // Bounding boxes are always given as (minx, miny, maxx, maxy) = (west, south, east, north);
//! // the client swaps axes for WMS 1.3.0 + EPSG:4326 automatically.
//! let png = wms
//!     .get_map(&GetMap::new(["0"], (-78.0, 38.0, -77.0, 39.0), "EPSG:4326", 512, 512).format("image/png"))
//!     .await?;
//! std::fs::write("map.png", &png.body).ok();
//! # Ok(()) }
//! ```

use serde::{Deserialize, Serialize};
use url::Url;

use crate::crs::{AxisOrder, Crs};
use crate::error::{Error, Result, parse_xml_checked, sniff_exception};
use crate::http::{Client, Kvp, Response, build_url, clean_service_url, fmt_num};
use crate::ows::{
    BoundingBox, MetadataLink, Operation, ServiceIdentification, ServiceProvider, keywords,
};
use crate::xml::Element;

/// WMS protocol version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WmsVersion {
    /// 1.1.1 (`SRS`, x/y axis order everywhere).
    V1_1_1,
    /// 1.3.0 (`CRS`, authority axis order).
    V1_3_0,
}

impl WmsVersion {
    /// Version string.
    pub fn as_str(&self) -> &'static str {
        match self {
            WmsVersion::V1_1_1 => "1.1.1",
            WmsVersion::V1_3_0 => "1.3.0",
        }
    }

    fn parse(s: &str) -> Option<WmsVersion> {
        match s.trim() {
            "1.1.1" | "1.1.0" | "1.1" => Some(WmsVersion::V1_1_1),
            "1.3.0" | "1.3" => Some(WmsVersion::V1_3_0),
            _ => None,
        }
    }
}

/// Service-level metadata of a WMS (`<Service>`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WmsService {
    /// `Name` (usually `WMS` or `OGC:WMS`).
    pub name: Option<String>,
    /// Title.
    pub title: Option<String>,
    /// Abstract.
    pub abstract_: Option<String>,
    /// Keywords.
    pub keywords: Vec<String>,
    /// Online resource URL.
    pub online_resource: Option<String>,
    /// Fees.
    pub fees: Option<String>,
    /// Access constraints.
    pub access_constraints: Option<String>,
    /// `LayerLimit` (1.3.0).
    pub layer_limit: Option<u32>,
    /// `MaxWidth` (1.3.0).
    pub max_width: Option<u32>,
    /// `MaxHeight` (1.3.0).
    pub max_height: Option<u32>,
}

/// A legend graphic of a style.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LegendUrl {
    /// MIME type.
    pub format: Option<String>,
    /// Width in pixels.
    pub width: Option<u32>,
    /// Height in pixels.
    pub height: Option<u32>,
    /// URL of the graphic.
    pub url: String,
}

/// A named style of a layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Style {
    /// Name used in the `STYLES` parameter.
    pub name: String,
    /// Title.
    pub title: Option<String>,
    /// Abstract.
    pub abstract_: Option<String>,
    /// Legend graphics.
    pub legend_urls: Vec<LegendUrl>,
}

/// A layer dimension such as `time` or `elevation`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dimension {
    /// Dimension name (lower-case in practice: `time`, `elevation`, ...).
    pub name: String,
    /// Units (`ISO8601`, `EPSG:5030`, ...).
    pub units: Option<String>,
    /// Unit symbol.
    pub unit_symbol: Option<String>,
    /// Default value.
    pub default: Option<String>,
    /// Whether multiple values may be requested.
    pub multiple_values: Option<bool>,
    /// Whether the nearest value is used.
    pub nearest_value: Option<bool>,
    /// Whether `current` is supported.
    pub current: Option<bool>,
    /// Raw extent string (`1980-01-01/2023-11-01/P1M,...` or a comma list).
    pub values: Option<String>,
}

impl Dimension {
    /// Splits [`Dimension::values`] at commas.
    pub fn value_list(&self) -> Vec<String> {
        self.values
            .as_deref()
            .map(|v| {
                v.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Attribution of a layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attribution {
    /// Title.
    pub title: Option<String>,
    /// URL.
    pub url: Option<String>,
    /// Logo URL.
    pub logo_url: Option<String>,
}

/// A WMS layer. Inherited properties (CRS, styles, bounding boxes,
/// dimensions, ...) are already resolved from parent layers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    /// Name (absent for category-only layers).
    pub name: Option<String>,
    /// Title.
    pub title: Option<String>,
    /// Abstract.
    pub abstract_: Option<String>,
    /// Keywords.
    pub keywords: Vec<String>,
    /// Supports GetFeatureInfo.
    pub queryable: bool,
    /// Opaque.
    pub opaque: bool,
    /// Cascade count.
    pub cascaded: Option<u32>,
    /// Supported CRSs (`CRS` in 1.3.0, `SRS` in 1.1.1), including inherited ones.
    pub crs: Vec<String>,
    /// WGS 84 extent (`EX_GeographicBoundingBox` / `LatLonBoundingBox`), lon/lat order.
    pub geographic_bbox: Option<BoundingBox>,
    /// Bounding boxes per CRS (stored as in the document, see [`BoundingBox::to_xy`]).
    pub bounding_boxes: Vec<BoundingBox>,
    /// Styles, including inherited ones.
    pub styles: Vec<Style>,
    /// Dimensions (time, elevation, ...).
    pub dimensions: Vec<Dimension>,
    /// Attribution.
    pub attribution: Option<Attribution>,
    /// Metadata URLs.
    pub metadata_urls: Vec<MetadataLink>,
    /// Data URLs.
    pub data_urls: Vec<MetadataLink>,
    /// Minimum scale denominator (1.3.0) or `ScaleHint/@min` (1.1.1).
    pub min_scale_denominator: Option<f64>,
    /// Maximum scale denominator (1.3.0) or `ScaleHint/@max` (1.1.1).
    pub max_scale_denominator: Option<f64>,
    /// Child layers.
    pub children: Vec<Layer>,
}

impl Layer {
    /// Bounding box for a CRS (matched regardless of spelling), x/y order.
    pub fn bbox_for(&self, crs: &str, version: WmsVersion) -> Option<(f64, f64, f64, f64)> {
        let want = Crs::parse(crs)?;
        self.bounding_boxes
            .iter()
            .find(|b| b.crs().map(|c| c.same_as(&want)).unwrap_or(false))
            .map(|b| b.to_xy(version == WmsVersion::V1_3_0))
    }

    /// Whether the layer advertises the given CRS.
    pub fn supports_crs(&self, crs: &str) -> bool {
        match Crs::parse(crs) {
            Some(want) => self
                .crs
                .iter()
                .filter_map(|c| Crs::parse(c))
                .any(|c| c.same_as(&want)),
            None => self.crs.iter().any(|c| c.eq_ignore_ascii_case(crs)),
        }
    }

    /// A dimension by (case-insensitive) name.
    pub fn dimension(&self, name: &str) -> Option<&Dimension> {
        self.dimensions
            .iter()
            .find(|d| d.name.eq_ignore_ascii_case(name))
    }

    fn walk<'a>(&'a self, out: &mut Vec<&'a Layer>) {
        out.push(self);
        for c in &self.children {
            c.walk(out);
        }
    }
}

/// Parsed WMS capabilities document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WmsCapabilities {
    /// Version the server answered with.
    pub version: WmsVersion,
    /// `updateSequence` attribute.
    pub update_sequence: Option<String>,
    /// Service metadata.
    pub service: WmsService,
    /// Service metadata in the common OWS shape (same data as `service`).
    pub identification: ServiceIdentification,
    /// Provider and contact information.
    pub provider: ServiceProvider,
    /// Operations (`GetCapabilities`, `GetMap`, `GetFeatureInfo`, vendor ones).
    pub operations: Vec<Operation>,
    /// Exception formats.
    pub exception_formats: Vec<String>,
    /// Top-level layers (usually one root layer).
    pub layers: Vec<Layer>,
}

impl WmsCapabilities {
    /// Parses a WMS 1.1.1 or 1.3.0 capabilities document.
    pub fn parse(bytes: &[u8]) -> Result<WmsCapabilities> {
        let root = parse_xml_checked(bytes)?;
        if !(root.is("WMS_Capabilities") || root.is("WMT_MS_Capabilities")) {
            return Err(Error::Parse(format!(
                "expected WMS_Capabilities, found <{}>",
                root.name
            )));
        }
        let version = root.attr("version").and_then(WmsVersion::parse).unwrap_or(
            if root.is("WMS_Capabilities") {
                WmsVersion::V1_3_0
            } else {
                WmsVersion::V1_1_1
            },
        );
        let svc = root
            .child("Service")
            .ok_or_else(|| Error::Parse("missing <Service>".into()))?;
        let num = |n: &str| svc.child_text(n).and_then(|t| t.parse().ok());
        let service = WmsService {
            name: svc.child_text("Name"),
            title: svc.child_text("Title"),
            abstract_: svc.child_text("Abstract"),
            keywords: keywords(svc),
            online_resource: svc
                .child("OnlineResource")
                .and_then(|o| o.attr("href"))
                .map(str::to_string),
            fees: svc.child_text("Fees"),
            access_constraints: svc.child_text("AccessConstraints"),
            layer_limit: num("LayerLimit"),
            max_width: num("MaxWidth"),
            max_height: num("MaxHeight"),
        };
        let identification = ServiceIdentification {
            service_type: service.name.clone(),
            versions: vec![version.as_str().to_string()],
            title: service.title.clone(),
            abstract_: service.abstract_.clone(),
            keywords: service.keywords.clone(),
            fees: service.fees.clone(),
            access_constraints: service.access_constraints.clone(),
            profiles: Vec::new(),
        };
        let provider = ServiceProvider::from_wms(svc);
        let cap = root
            .child("Capability")
            .ok_or_else(|| Error::Parse("missing <Capability>".into()))?;
        let operations = cap
            .child("Request")
            .map(|r| r.children.iter().map(Operation::from_wms).collect())
            .unwrap_or_default();
        let exception_formats = cap
            .child("Exception")
            .map(|e| e.children_text("Format"))
            .unwrap_or_default();
        let base = Layer::default();
        let layers = cap
            .children_named("Layer")
            .map(|l| parse_layer(l, &base))
            .collect();
        Ok(WmsCapabilities {
            version,
            update_sequence: root.attr("updateSequence").map(str::to_string),
            service,
            identification,
            provider,
            operations,
            exception_formats,
            layers,
        })
    }

    /// All layers, depth first (including unnamed category layers).
    pub fn all_layers(&self) -> Vec<&Layer> {
        let mut out = Vec::new();
        for l in &self.layers {
            l.walk(&mut out);
        }
        out
    }

    /// Layers that have a `Name` (i.e. can be requested), depth first.
    /// This is OWSLib's `contents`.
    pub fn named_layers(&self) -> Vec<&Layer> {
        self.all_layers()
            .into_iter()
            .filter(|l| l.name.is_some())
            .collect()
    }

    /// Looks up a layer by name.
    pub fn layer(&self, name: &str) -> Option<&Layer> {
        self.all_layers()
            .into_iter()
            .find(|l| l.name.as_deref() == Some(name))
    }

    /// Looks up an operation by name (case-insensitive).
    pub fn operation(&self, name: &str) -> Option<&Operation> {
        self.operations
            .iter()
            .find(|o| o.name.eq_ignore_ascii_case(name))
    }
}

fn parse_style(s: &Element) -> Option<Style> {
    Some(Style {
        name: s.child_text("Name")?,
        title: s.child_text("Title"),
        abstract_: s.child_text("Abstract"),
        legend_urls: s
            .children_named("LegendURL")
            .filter_map(|l| {
                Some(LegendUrl {
                    format: l.child_text("Format"),
                    width: l.attr_u32("width"),
                    height: l.attr_u32("height"),
                    url: l.child("OnlineResource")?.attr("href")?.to_string(),
                })
            })
            .collect(),
    })
}

fn parse_layer(e: &Element, parent: &Layer) -> Layer {
    // Inherited (additive): CRS, styles.
    let mut crs = parent.crs.clone();
    for c in e.children.iter().filter(|c| c.is("CRS") || c.is("SRS")) {
        if let Some(t) = c.text_opt() {
            // WMS 1.1.1 allows several codes separated by whitespace.
            for code in t.split_whitespace() {
                if !crs.iter().any(|x| x.eq_ignore_ascii_case(code)) {
                    crs.push(code.to_string());
                }
            }
        }
    }
    let mut styles = parent.styles.clone();
    for s in e.children_named("Style").filter_map(parse_style) {
        styles.retain(|x| x.name != s.name);
        styles.push(s);
    }

    // Inherited (replace): bbox, geographic bbox, dimensions, attribution, scale.
    let geographic_bbox = e
        .child("EX_GeographicBoundingBox")
        .and_then(BoundingBox::from_ex_geographic)
        .or_else(|| {
            e.child("LatLonBoundingBox")
                .and_then(BoundingBox::from_attrs)
                .map(|mut b| {
                    b.crs = Some("CRS:84".into());
                    b
                })
        })
        .or_else(|| parent.geographic_bbox.clone());
    let mut bounding_boxes = parent.bounding_boxes.clone();
    for b in e
        .children_named("BoundingBox")
        .filter_map(BoundingBox::from_attrs)
    {
        bounding_boxes.retain(|x| x.crs != b.crs);
        bounding_boxes.push(b);
    }

    let mut dimensions = parent.dimensions.clone();
    for d in e.children_named("Dimension") {
        let name = d.attr("name").unwrap_or_default().to_string();
        let dim = Dimension {
            name: name.clone(),
            units: d.attr("units").map(str::to_string),
            unit_symbol: d.attr("unitSymbol").map(str::to_string),
            default: d.attr("default").map(str::to_string),
            multiple_values: d.attr_bool("multipleValues"),
            nearest_value: d.attr_bool("nearestValue"),
            current: d.attr_bool("current"),
            values: d.text_opt(),
        };
        dimensions.retain(|x| !x.name.eq_ignore_ascii_case(&name));
        dimensions.push(dim);
    }
    // WMS 1.1.1: values, default etc. live in <Extent name="...">.
    for x in e.children_named("Extent") {
        let name = x.attr("name").unwrap_or_default();
        if let Some(d) = dimensions
            .iter_mut()
            .find(|d| d.name.eq_ignore_ascii_case(name))
        {
            d.values = x.text_opt().or(d.values.take());
            if let Some(def) = x.attr("default") {
                d.default = Some(def.to_string());
            }
            if let Some(v) = x.attr_bool("multipleValues") {
                d.multiple_values = Some(v);
            }
            if let Some(v) = x.attr_bool("nearestValue") {
                d.nearest_value = Some(v);
            }
            if let Some(v) = x.attr_bool("current") {
                d.current = Some(v);
            }
        }
    }

    let attribution = e
        .child("Attribution")
        .map(|a| Attribution {
            title: a.child_text("Title"),
            url: a
                .child("OnlineResource")
                .and_then(|o| o.attr("href"))
                .map(str::to_string),
            logo_url: a
                .path(&["LogoURL", "OnlineResource"])
                .and_then(|o| o.attr("href"))
                .map(str::to_string),
        })
        .or_else(|| parent.attribution.clone());

    let scale = |n: &str| e.child_text(n).and_then(|t| t.parse::<f64>().ok());
    let hint = e.child("ScaleHint");
    let min_scale_denominator = scale("MinScaleDenominator")
        .or_else(|| hint.and_then(|h| h.attr_f64("min")))
        .or(parent.min_scale_denominator);
    let max_scale_denominator = scale("MaxScaleDenominator")
        .or_else(|| hint.and_then(|h| h.attr_f64("max")))
        .or(parent.max_scale_denominator);

    let mut layer = Layer {
        name: e.child_text("Name"),
        title: e.child_text("Title"),
        abstract_: e.child_text("Abstract"),
        keywords: keywords(e),
        queryable: e.attr_bool("queryable").unwrap_or(parent.queryable),
        opaque: e.attr_bool("opaque").unwrap_or(parent.opaque),
        cascaded: e.attr_u32("cascaded").or(parent.cascaded),
        crs,
        geographic_bbox,
        bounding_boxes,
        styles,
        dimensions,
        attribution,
        metadata_urls: e
            .children_named("MetadataURL")
            .filter_map(MetadataLink::from_wms)
            .collect(),
        data_urls: e
            .children_named("DataURL")
            .filter_map(MetadataLink::from_wms)
            .collect(),
        min_scale_denominator,
        max_scale_denominator,
        children: Vec::new(),
    };
    let children = e
        .children_named("Layer")
        .map(|c| parse_layer(c, &layer))
        .collect();
    layer.children = children;
    layer
}

/// How bounding boxes passed to request builders are interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AxisPolicy {
    /// The bbox is (west/minx, south/miny, east/maxx, north/maxy); the client
    /// swaps it for WMS 1.3.0 when the CRS has y/x axis order.
    #[default]
    Auto,
    /// Send the numbers exactly as given.
    AsGiven,
}

/// Formats a bbox for a request, applying the version's axis-order rules.
fn wms_bbox(
    bbox: (f64, f64, f64, f64),
    crs: &str,
    version: WmsVersion,
    policy: AxisPolicy,
) -> String {
    let (a, b, c, d) = bbox;
    let swap = policy == AxisPolicy::Auto
        && version == WmsVersion::V1_3_0
        && Crs::parse(crs)
            .map(|c| c.authority_axis_order() == AxisOrder::YX)
            .unwrap_or(false);
    let v = if swap { [b, a, d, c] } else { [a, b, c, d] };
    v.iter().map(|x| fmt_num(*x)).collect::<Vec<_>>().join(",")
}

/// A GetMap request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GetMap {
    /// Layer names.
    pub layers: Vec<String>,
    /// Style names (one per layer, or empty for defaults).
    pub styles: Vec<String>,
    /// `(minx, miny, maxx, maxy)`, see [`AxisPolicy`].
    pub bbox: (f64, f64, f64, f64),
    /// CRS code, e.g. `EPSG:4326`.
    pub crs: String,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Output MIME type (`image/png` if `None`).
    pub format: Option<String>,
    /// `TRANSPARENT=TRUE`.
    pub transparent: Option<bool>,
    /// Background colour, e.g. `0xFFFFFF`.
    pub bgcolor: Option<String>,
    /// `TIME` dimension value.
    pub time: Option<String>,
    /// `ELEVATION` dimension value.
    pub elevation: Option<String>,
    /// Exception format.
    pub exceptions: Option<String>,
    /// Axis order handling for `bbox`.
    pub axis_policy: AxisPolicy,
    /// Other dimensions (sent as `DIM_<NAME>`) and vendor parameters.
    pub extra: Vec<(String, String)>,
}

impl GetMap {
    /// A request for `layers` over `bbox` (west, south, east, north in `crs` units).
    pub fn new<I, S>(
        layers: I,
        bbox: (f64, f64, f64, f64),
        crs: &str,
        width: u32,
        height: u32,
    ) -> GetMap
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        GetMap {
            layers: layers.into_iter().map(Into::into).collect(),
            styles: Vec::new(),
            bbox,
            crs: crs.to_string(),
            width,
            height,
            format: None,
            transparent: None,
            bgcolor: None,
            time: None,
            elevation: None,
            exceptions: None,
            axis_policy: AxisPolicy::Auto,
            extra: Vec::new(),
        }
    }

    /// Sets the output format.
    pub fn format(mut self, f: &str) -> GetMap {
        self.format = Some(f.to_string());
        self
    }
    /// Sets styles.
    pub fn styles<I: IntoIterator<Item = S>, S: Into<String>>(mut self, s: I) -> GetMap {
        self.styles = s.into_iter().map(Into::into).collect();
        self
    }
    /// Requests a transparent background.
    pub fn transparent(mut self, t: bool) -> GetMap {
        self.transparent = Some(t);
        self
    }
    /// Sets `BGCOLOR`.
    pub fn bgcolor(mut self, c: &str) -> GetMap {
        self.bgcolor = Some(c.to_string());
        self
    }
    /// Sets `TIME`.
    pub fn time(mut self, t: &str) -> GetMap {
        self.time = Some(t.to_string());
        self
    }
    /// Sets `ELEVATION`.
    pub fn elevation(mut self, e: &str) -> GetMap {
        self.elevation = Some(e.to_string());
        self
    }
    /// Sets a sample dimension (`DIM_<NAME>`).
    pub fn dimension(mut self, name: &str, value: &str) -> GetMap {
        self.extra.push((
            format!("DIM_{}", name.to_ascii_uppercase()),
            value.to_string(),
        ));
        self
    }
    /// Adds a vendor parameter.
    pub fn param(mut self, k: &str, v: &str) -> GetMap {
        self.extra.push((k.to_string(), v.to_string()));
        self
    }
    /// Sets the axis policy.
    pub fn axis_policy(mut self, p: AxisPolicy) -> GetMap {
        self.axis_policy = p;
        self
    }

    fn kvp(&self, version: WmsVersion, request: &str) -> Kvp {
        let mut k = Kvp::new();
        k.push("SERVICE", "WMS")
            .push("VERSION", version.as_str())
            .push("REQUEST", request)
            .push("LAYERS", self.layers.join(","))
            .push("STYLES", self.styles.join(","));
        let crs_key = if version == WmsVersion::V1_3_0 {
            "CRS"
        } else {
            "SRS"
        };
        k.push(crs_key, self.crs.clone())
            .push(
                "BBOX",
                wms_bbox(self.bbox, &self.crs, version, self.axis_policy),
            )
            .push("WIDTH", self.width.to_string())
            .push("HEIGHT", self.height.to_string())
            .push(
                "FORMAT",
                self.format.clone().unwrap_or_else(|| "image/png".into()),
            )
            .opt(
                "TRANSPARENT",
                self.transparent.map(|t| if t { "TRUE" } else { "FALSE" }),
            )
            .opt("BGCOLOR", self.bgcolor.clone())
            .opt("TIME", self.time.clone())
            .opt("ELEVATION", self.elevation.clone())
            .opt("EXCEPTIONS", self.exceptions.clone());
        k.extend(&self.extra);
        k
    }
}

/// A GetFeatureInfo request: a [`GetMap`] plus a query point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GetFeatureInfo {
    /// The map the pixel refers to.
    pub map: GetMap,
    /// Layers to query (defaults to the map layers).
    pub query_layers: Vec<String>,
    /// Pixel column (`I` in 1.3.0, `X` in 1.1.1).
    pub i: u32,
    /// Pixel row (`J` in 1.3.0, `Y` in 1.1.1).
    pub j: u32,
    /// `INFO_FORMAT`, e.g. `application/json`, `text/xml`, `text/html`.
    pub info_format: Option<String>,
    /// `FEATURE_COUNT`.
    pub feature_count: Option<u32>,
}

impl GetFeatureInfo {
    /// Queries pixel `(i, j)` of `map`.
    pub fn new(map: GetMap, i: u32, j: u32) -> GetFeatureInfo {
        GetFeatureInfo {
            query_layers: map.layers.clone(),
            map,
            i,
            j,
            info_format: None,
            feature_count: None,
        }
    }
    /// Sets `INFO_FORMAT`.
    pub fn info_format(mut self, f: &str) -> GetFeatureInfo {
        self.info_format = Some(f.to_string());
        self
    }
    /// Sets `FEATURE_COUNT`.
    pub fn feature_count(mut self, n: u32) -> GetFeatureInfo {
        self.feature_count = Some(n);
        self
    }
    /// Sets the queried layers.
    pub fn query_layers<I: IntoIterator<Item = S>, S: Into<String>>(
        mut self,
        l: I,
    ) -> GetFeatureInfo {
        self.query_layers = l.into_iter().map(Into::into).collect();
        self
    }

    fn kvp(&self, version: WmsVersion) -> Kvp {
        let mut k = self.map.kvp(version, "GetFeatureInfo");
        let (ik, jk) = if version == WmsVersion::V1_3_0 {
            ("I", "J")
        } else {
            ("X", "Y")
        };
        k.0.retain(|(n, _)| n != "FORMAT" || self.map.format.is_some());
        k.push("QUERY_LAYERS", self.query_layers.join(","))
            .push(ik, self.i.to_string())
            .push(jk, self.j.to_string())
            .push(
                "INFO_FORMAT",
                self.info_format
                    .clone()
                    .unwrap_or_else(|| "text/plain".into()),
            )
            .opt("FEATURE_COUNT", self.feature_count.map(|n| n.to_string()));
        k
    }
}

/// A non-capabilities response (map image, legend, feature info).
#[derive(Debug, Clone)]
pub struct WmsResponse {
    /// MIME type reported by the server.
    pub content_type: Option<String>,
    /// Raw body.
    pub body: bytes::Bytes,
}

impl WmsResponse {
    /// Body as UTF-8 text (lossy).
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// Body parsed as JSON.
    pub fn json(&self) -> Result<serde_json::Value> {
        Ok(serde_json::from_slice(&self.body)?)
    }

    /// Body parsed as a GeoJSON feature collection (for
    /// `INFO_FORMAT=application/json` / `application/geo+json`).
    pub fn geojson(&self) -> Result<geojson::FeatureCollection> {
        Ok(serde_json::from_slice(&self.body)?)
    }

    /// Body parsed as an XML tree.
    pub fn xml(&self) -> Result<Element> {
        Element::parse(&self.body)
    }

    pub(crate) fn from_http(r: Response) -> Result<WmsResponse> {
        // Servers often return exceptions with status 200 and even an image MIME type.
        let looks_xml = r.is_xml() || r.body.starts_with(b"<?xml") || r.body.starts_with(b"<");
        if looks_xml && let Some(err) = sniff_exception(&r.body) {
            return Err(err);
        }
        Ok(WmsResponse {
            content_type: r.content_type,
            body: r.body,
        })
    }
}

/// A connected WMS: capabilities plus an HTTP client.
#[derive(Debug, Clone)]
pub struct WebMapService {
    url: Url,
    client: Client,
    capabilities: WmsCapabilities,
    use_capabilities_urls: bool,
}

impl WebMapService {
    /// Fetches and parses capabilities using a default [`Client`].
    pub async fn connect(url: &str, version: WmsVersion) -> Result<WebMapService> {
        Self::connect_with(Client::new(), url, version).await
    }

    /// Fetches and parses capabilities using the given client.
    pub async fn connect_with(
        client: Client,
        url: &str,
        version: WmsVersion,
    ) -> Result<WebMapService> {
        let caps_url = Self::capabilities_url(url, version)?;
        let resp = client
            .get(
                &caps_url,
                Some("application/vnd.ogc.wms_xml, text/xml, application/xml"),
            )
            .await?;
        let capabilities = WmsCapabilities::parse(&resp.body)?;
        Ok(WebMapService {
            url: clean_service_url(url)?,
            client,
            capabilities,
            use_capabilities_urls: true,
        })
    }

    /// Builds a service from an already downloaded capabilities document.
    pub fn from_capabilities(client: Client, url: &str, xml: &[u8]) -> Result<WebMapService> {
        Ok(WebMapService {
            url: clean_service_url(url)?,
            client,
            capabilities: WmsCapabilities::parse(xml)?,
            use_capabilities_urls: true,
        })
    }

    /// The GetCapabilities URL for a service endpoint.
    pub fn capabilities_url(url: &str, version: WmsVersion) -> Result<Url> {
        let mut k = Kvp::new();
        k.push("SERVICE", "WMS")
            .push("REQUEST", "GetCapabilities")
            .push("VERSION", version.as_str());
        build_url(clean_service_url(url)?.as_str(), &k.0)
    }

    /// Ignore operation URLs from the capabilities and always use the URL the
    /// service was opened with (useful when servers advertise internal hosts).
    pub fn use_service_url(mut self) -> WebMapService {
        self.use_capabilities_urls = false;
        self
    }

    /// Parsed capabilities.
    pub fn capabilities(&self) -> &WmsCapabilities {
        &self.capabilities
    }

    /// Negotiated version.
    pub fn version(&self) -> WmsVersion {
        self.capabilities.version
    }

    /// The HTTP client.
    pub fn client(&self) -> &Client {
        &self.client
    }

    fn endpoint(&self, op: &str) -> String {
        if self.use_capabilities_urls
            && let Some(u) = self.capabilities.operation(op).and_then(|o| o.get_url())
        {
            return u.to_string();
        }
        self.url.to_string()
    }

    fn check_layers(&self, layers: &[String]) -> Result<()> {
        for l in layers {
            if self.capabilities.layer(l).is_none() {
                return Err(Error::InvalidRequest(format!("unknown layer: {l}")));
            }
        }
        Ok(())
    }

    /// The GetMap URL for a request (no network access).
    pub fn get_map_url(&self, req: &GetMap) -> Result<Url> {
        build_url(
            &self.endpoint("GetMap"),
            &req.kvp(self.version(), "GetMap").0,
        )
    }

    /// Requests a map image.
    pub async fn get_map(&self, req: &GetMap) -> Result<WmsResponse> {
        self.check_layers(&req.layers)?;
        let url = self.get_map_url(req)?;
        WmsResponse::from_http(self.client.get(&url, None).await?)
    }

    /// The GetFeatureInfo URL for a request (no network access).
    pub fn get_feature_info_url(&self, req: &GetFeatureInfo) -> Result<Url> {
        build_url(&self.endpoint("GetFeatureInfo"), &req.kvp(self.version()).0)
    }

    /// Queries features at a pixel.
    pub async fn get_feature_info(&self, req: &GetFeatureInfo) -> Result<WmsResponse> {
        if self.capabilities.operation("GetFeatureInfo").is_none() {
            return Err(Error::Unsupported("GetFeatureInfo".into()));
        }
        self.check_layers(&req.query_layers)?;
        let url = self.get_feature_info_url(req)?;
        WmsResponse::from_http(self.client.get(&url, None).await?)
    }

    /// Downloads the first legend graphic of a layer's style (default style if `None`).
    pub async fn get_legend(&self, layer: &str, style: Option<&str>) -> Result<WmsResponse> {
        let l = self
            .capabilities
            .layer(layer)
            .ok_or_else(|| Error::InvalidRequest(format!("unknown layer: {layer}")))?;
        let s = match style {
            Some(name) => l.styles.iter().find(|s| s.name == name),
            None => l.styles.first(),
        }
        .ok_or_else(|| Error::Unsupported(format!("no style for layer {layer}")))?;
        let lg = s
            .legend_urls
            .first()
            .ok_or_else(|| Error::Unsupported("style has no LegendURL".into()))?;
        let url = Url::parse(&lg.url)?;
        WmsResponse::from_http(self.client.get(&url, None).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bbox_axis_order() {
        let b = (-10.0, 40.0, 5.0, 50.0);
        assert_eq!(
            wms_bbox(b, "EPSG:4326", WmsVersion::V1_3_0, AxisPolicy::Auto),
            "40,-10,50,5"
        );
        assert_eq!(
            wms_bbox(b, "EPSG:4326", WmsVersion::V1_1_1, AxisPolicy::Auto),
            "-10,40,5,50"
        );
        assert_eq!(
            wms_bbox(b, "CRS:84", WmsVersion::V1_3_0, AxisPolicy::Auto),
            "-10,40,5,50"
        );
        assert_eq!(
            wms_bbox(b, "EPSG:4326", WmsVersion::V1_3_0, AxisPolicy::AsGiven),
            "-10,40,5,50"
        );
        assert_eq!(
            wms_bbox(b, "EPSG:3857", WmsVersion::V1_3_0, AxisPolicy::Auto),
            "-10,40,5,50"
        );
    }
}
