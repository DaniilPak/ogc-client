//! Web Map Tile Service (WMTS) 1.0.0 with KVP and RESTful tile access.
//!
//! ```no_run
//! # async fn run() -> ogc_client::Result<()> {
//! use ogc_client::wmts::{GetTile, WebMapTileService};
//!
//! let wmts = WebMapTileService::connect(
//!     "https://basemap.nationalmap.gov/arcgis/rest/services/USGSTopo/MapServer/WMTS/1.0.0/WMTSCapabilities.xml",
//! )
//! .await?;
//! let layer = &wmts.capabilities().layers[0];
//! let tms = wmts.capabilities().tile_matrix_set("default028mm").unwrap();
//! // Which tile covers Washington, DC at level 10?
//! let (x, y) = (-8575605.0, 4707174.0); // EPSG:3857
//! let idx = tms.tile_at("10", x, y).unwrap();
//! let tile = wmts
//!     .get_tile(&GetTile::new(&layer.identifier, "default028mm", "10", idx.row, idx.col))
//!     .await?;
//! # Ok(()) }
//! ```

use serde::{Deserialize, Serialize};
use url::Url;

use crate::crs::Crs;
use crate::error::{Error, Result, parse_xml_checked};
use crate::http::{Client, Kvp, build_url, clean_service_url};
use crate::ows::{
    BoundingBox, MetadataLink, Operation, ServiceIdentification, ServiceProvider, keywords,
    operations_from_ows,
};
use crate::tms::{TileMatrix, TileMatrixSet, normalize_corner};
use crate::wms::{LegendUrl, WmsResponse};
use crate::xml::{Element, parse_numbers};

/// A WMTS style.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WmtsStyle {
    /// Identifier.
    pub identifier: String,
    /// Title.
    pub title: Option<String>,
    /// Whether this is the default style.
    pub is_default: bool,
    /// Legends.
    pub legend_urls: Vec<LegendUrl>,
}

/// A WMTS dimension (e.g. `Time`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WmtsDimension {
    /// Identifier (`Time`, `Elevation`, ...).
    pub identifier: String,
    /// Title.
    pub title: Option<String>,
    /// Unit of measure.
    pub uom: Option<String>,
    /// Unit symbol.
    pub unit_symbol: Option<String>,
    /// Default value.
    pub default: Option<String>,
    /// Whether `current` is supported.
    pub current: Option<bool>,
    /// Allowed values / ranges.
    pub values: Vec<String>,
}

/// Limits of a tile matrix for a layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TileMatrixLimits {
    /// Tile matrix identifier.
    pub tile_matrix: String,
    /// Minimum row.
    pub min_row: u64,
    /// Maximum row.
    pub max_row: u64,
    /// Minimum column.
    pub min_col: u64,
    /// Maximum column.
    pub max_col: u64,
}

/// Link from a layer to a tile matrix set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TileMatrixSetLink {
    /// Tile matrix set identifier.
    pub tile_matrix_set: String,
    /// Optional per-level limits.
    pub limits: Vec<TileMatrixLimits>,
}

/// A RESTful URL template (`ResourceURL`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceUrl {
    /// MIME type.
    pub format: String,
    /// `tile` or `FeatureInfo` (or vendor types).
    pub resource_type: String,
    /// Template with `{TileMatrixSet}`, `{TileMatrix}`, `{TileRow}`, `{TileCol}`, `{Style}` and dimension placeholders.
    pub template: String,
}

/// A WMTS layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WmtsLayer {
    /// Identifier used in requests.
    pub identifier: String,
    /// Title.
    pub title: Option<String>,
    /// Abstract.
    pub abstract_: Option<String>,
    /// Keywords.
    pub keywords: Vec<String>,
    /// WGS 84 bounding box (lon/lat).
    pub wgs84_bbox: Option<BoundingBox>,
    /// Other bounding boxes (in CRS axis order).
    pub bounding_boxes: Vec<BoundingBox>,
    /// Styles.
    pub styles: Vec<WmtsStyle>,
    /// Tile formats.
    pub formats: Vec<String>,
    /// FeatureInfo formats.
    pub info_formats: Vec<String>,
    /// Dimensions.
    pub dimensions: Vec<WmtsDimension>,
    /// Tile matrix set links.
    pub tile_matrix_set_links: Vec<TileMatrixSetLink>,
    /// RESTful templates.
    pub resource_urls: Vec<ResourceUrl>,
    /// `ows:Metadata` links.
    pub metadata: Vec<MetadataLink>,
}

impl WmtsLayer {
    /// The default style identifier (or the first one).
    pub fn default_style(&self) -> Option<&str> {
        self.styles
            .iter()
            .find(|s| s.is_default)
            .or_else(|| self.styles.first())
            .map(|s| s.identifier.as_str())
    }

    /// Whether the layer is available in a tile matrix set.
    pub fn has_tile_matrix_set(&self, id: &str) -> bool {
        self.tile_matrix_set_links
            .iter()
            .any(|l| l.tile_matrix_set == id)
    }
}

/// Parsed WMTS capabilities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WmtsCapabilities {
    /// Version (`1.0.0`).
    pub version: String,
    /// Service identification.
    pub identification: ServiceIdentification,
    /// Service provider.
    pub provider: ServiceProvider,
    /// Operations.
    pub operations: Vec<Operation>,
    /// Layers.
    pub layers: Vec<WmtsLayer>,
    /// Tile matrix sets (top-left corners normalized to x/y).
    pub tile_matrix_sets: Vec<TileMatrixSet>,
    /// `ServiceMetadataURL` (RESTful capabilities).
    pub service_metadata_url: Option<String>,
}

impl WmtsCapabilities {
    /// Parses a WMTS 1.0.0 capabilities document.
    pub fn parse(bytes: &[u8]) -> Result<WmtsCapabilities> {
        let root = parse_xml_checked(bytes)?;
        if !root.is("Capabilities") {
            return Err(Error::Parse(format!(
                "expected WMTS Capabilities, found <{}>",
                root.name
            )));
        }
        let contents = root
            .child("Contents")
            .ok_or_else(|| Error::Parse("missing <Contents>".into()))?;
        Ok(WmtsCapabilities {
            version: root.attr("version").unwrap_or("1.0.0").to_string(),
            identification: root
                .child("ServiceIdentification")
                .map(ServiceIdentification::from_ows)
                .unwrap_or_default(),
            provider: root
                .child("ServiceProvider")
                .map(ServiceProvider::from_ows)
                .unwrap_or_default(),
            operations: root
                .child("OperationsMetadata")
                .map(operations_from_ows)
                .unwrap_or_default(),
            layers: contents.children_named("Layer").map(parse_layer).collect(),
            tile_matrix_sets: contents
                .children_named("TileMatrixSet")
                .filter_map(parse_tms)
                .collect(),
            service_metadata_url: root
                .child("ServiceMetadataURL")
                .and_then(|e| e.attr("href"))
                .map(str::to_string),
        })
    }

    /// Layer by identifier.
    pub fn layer(&self, id: &str) -> Option<&WmtsLayer> {
        self.layers.iter().find(|l| l.identifier == id)
    }

    /// Tile matrix set by identifier.
    pub fn tile_matrix_set(&self, id: &str) -> Option<&TileMatrixSet> {
        self.tile_matrix_sets.iter().find(|t| t.identifier == id)
    }

    /// Operation by name.
    pub fn operation(&self, name: &str) -> Option<&Operation> {
        self.operations
            .iter()
            .find(|o| o.name.eq_ignore_ascii_case(name))
    }
}

fn parse_layer(e: &Element) -> WmtsLayer {
    WmtsLayer {
        identifier: e.child_text("Identifier").unwrap_or_default(),
        title: e.child_text("Title"),
        abstract_: e.child_text("Abstract"),
        keywords: keywords(e),
        wgs84_bbox: e.child("WGS84BoundingBox").and_then(BoundingBox::from_ows),
        bounding_boxes: e
            .children_named("BoundingBox")
            .filter_map(BoundingBox::from_ows)
            .collect(),
        styles: e
            .children_named("Style")
            .map(|s| WmtsStyle {
                identifier: s.child_text("Identifier").unwrap_or_default(),
                title: s.child_text("Title"),
                is_default: s.attr_bool("isDefault").unwrap_or(false),
                legend_urls: s
                    .children_named("LegendURL")
                    .filter_map(|l| {
                        Some(LegendUrl {
                            format: l.attr("format").map(str::to_string),
                            width: l.attr_u32("width"),
                            height: l.attr_u32("height"),
                            url: l.attr("href")?.to_string(),
                        })
                    })
                    .collect(),
            })
            .collect(),
        formats: e.children_text("Format"),
        info_formats: e.children_text("InfoFormat"),
        dimensions: e
            .children_named("Dimension")
            .map(|d| WmtsDimension {
                identifier: d.child_text("Identifier").unwrap_or_default(),
                title: d.child_text("Title"),
                uom: d.child_text("UOM"),
                unit_symbol: d.child_text("UnitSymbol"),
                default: d.child_text("Default"),
                current: d.child_text("Current").map(|c| c == "true" || c == "1"),
                values: d.children_text("Value"),
            })
            .collect(),
        tile_matrix_set_links: e
            .children_named("TileMatrixSetLink")
            .map(|l| TileMatrixSetLink {
                tile_matrix_set: l.child_text("TileMatrixSet").unwrap_or_default(),
                limits: l
                    .child("TileMatrixSetLimits")
                    .map(|lim| {
                        lim.children_named("TileMatrixLimits")
                            .filter_map(|t| {
                                let n = |x: &str| t.child_text(x)?.parse().ok();
                                Some(TileMatrixLimits {
                                    tile_matrix: t.child_text("TileMatrix")?,
                                    min_row: n("MinTileRow")?,
                                    max_row: n("MaxTileRow")?,
                                    min_col: n("MinTileCol")?,
                                    max_col: n("MaxTileCol")?,
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            })
            .collect(),
        resource_urls: e
            .children_named("ResourceURL")
            .filter_map(|r| {
                Some(ResourceUrl {
                    format: r.attr("format").unwrap_or_default().to_string(),
                    resource_type: r.attr("resourceType").unwrap_or("tile").to_string(),
                    template: r.attr("template")?.to_string(),
                })
            })
            .collect(),
        metadata: e
            .children_named("Metadata")
            .filter_map(|m| {
                Some(MetadataLink {
                    type_: m.attr("role").map(str::to_string),
                    format: None,
                    url: m.attr("href")?.to_string(),
                })
            })
            .collect(),
    }
}

fn parse_tms(e: &Element) -> Option<TileMatrixSet> {
    let crs_str = e.child_text("SupportedCRS")?;
    let crs = Crs::parse(&crs_str);
    let bounding_box = e
        .child("BoundingBox")
        .and_then(BoundingBox::from_ows)
        .map(|b| {
            let (x0, y0) = normalize_corner(crs.as_ref(), b.minx, b.miny);
            let (x1, y1) = normalize_corner(crs.as_ref(), b.maxx, b.maxy);
            (x0, y0, x1, y1)
        });
    let tile_matrices = e
        .children_named("TileMatrix")
        .filter_map(|m| {
            let corner = parse_numbers(&m.child_text("TopLeftCorner")?);
            if corner.len() < 2 {
                return None;
            }
            let n = |x: &str| m.child_text(x)?.parse::<f64>().ok();
            Some(TileMatrix {
                identifier: m.child_text("Identifier")?,
                scale_denominator: n("ScaleDenominator")?,
                top_left: normalize_corner(crs.as_ref(), corner[0], corner[1]),
                tile_width: n("TileWidth")? as u32,
                tile_height: n("TileHeight")? as u32,
                matrix_width: n("MatrixWidth")? as u64,
                matrix_height: n("MatrixHeight")? as u64,
            })
        })
        .collect();
    Some(TileMatrixSet {
        identifier: e.child_text("Identifier")?,
        title: e.child_text("Title"),
        crs: crs_str,
        well_known_scale_set: e.child_text("WellKnownScaleSet"),
        bounding_box,
        tile_matrices,
    })
}

/// Which binding to use for GetTile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TileEncoding {
    /// RESTful if the layer has a matching `ResourceURL`, else KVP.
    #[default]
    Auto,
    /// Key-value pairs on the GetTile endpoint.
    Kvp,
    /// `ResourceURL` templates.
    Rest,
}

/// A GetTile request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GetTile {
    /// Layer identifier.
    pub layer: String,
    /// Style (default style if `None`).
    pub style: Option<String>,
    /// Tile format (first layer format if `None`).
    pub format: Option<String>,
    /// Tile matrix set.
    pub tile_matrix_set: String,
    /// Tile matrix (zoom level) identifier.
    pub tile_matrix: String,
    /// Row.
    pub row: u64,
    /// Column.
    pub col: u64,
    /// Dimension values (`Time` → `2024-01-01`); defaults are used for missing ones.
    pub dimensions: Vec<(String, String)>,
    /// Binding to use.
    pub encoding: TileEncoding,
}

impl GetTile {
    /// A tile request.
    pub fn new(
        layer: &str,
        tile_matrix_set: &str,
        tile_matrix: &str,
        row: u64,
        col: u64,
    ) -> GetTile {
        GetTile {
            layer: layer.to_string(),
            style: None,
            format: None,
            tile_matrix_set: tile_matrix_set.to_string(),
            tile_matrix: tile_matrix.to_string(),
            row,
            col,
            dimensions: Vec::new(),
            encoding: TileEncoding::Auto,
        }
    }
    /// Sets the style.
    pub fn style(mut self, s: &str) -> GetTile {
        self.style = Some(s.to_string());
        self
    }
    /// Sets the format.
    pub fn format(mut self, f: &str) -> GetTile {
        self.format = Some(f.to_string());
        self
    }
    /// Sets a dimension value.
    pub fn dimension(mut self, name: &str, value: &str) -> GetTile {
        self.dimensions.push((name.to_string(), value.to_string()));
        self
    }
    /// Forces a binding.
    pub fn encoding(mut self, e: TileEncoding) -> GetTile {
        self.encoding = e;
        self
    }
}

/// Replaces `{name}` placeholders case-insensitively.
fn fill_template(template: &str, values: &[(String, String)]) -> Option<String> {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let end = rest[start..].find('}')? + start;
        let key = &rest[start + 1..end];
        let v = values.iter().find(|(k, _)| k.eq_ignore_ascii_case(key))?;
        out.push_str(&v.1);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// A connected WMTS.
#[derive(Debug, Clone)]
pub struct WebMapTileService {
    url: Url,
    client: Client,
    capabilities: WmtsCapabilities,
}

impl WebMapTileService {
    /// Fetches capabilities. `url` may be a KVP endpoint or a RESTful
    /// `WMTSCapabilities.xml` URL.
    pub async fn connect(url: &str) -> Result<WebMapTileService> {
        Self::connect_with(Client::new(), url).await
    }

    /// Fetches capabilities with the given client.
    pub async fn connect_with(client: Client, url: &str) -> Result<WebMapTileService> {
        let caps_url = Self::capabilities_url(url)?;
        let resp = client
            .get(&caps_url, Some("application/xml, text/xml"))
            .await?;
        Ok(WebMapTileService {
            url: clean_service_url(url)?,
            client,
            capabilities: WmtsCapabilities::parse(&resp.body)?,
        })
    }

    /// Builds a service from a downloaded capabilities document.
    pub fn from_capabilities(client: Client, url: &str, xml: &[u8]) -> Result<WebMapTileService> {
        Ok(WebMapTileService {
            url: clean_service_url(url)?,
            client,
            capabilities: WmtsCapabilities::parse(xml)?,
        })
    }

    /// The GetCapabilities URL (RESTful URLs ending in `.xml` are used as-is).
    pub fn capabilities_url(url: &str) -> Result<Url> {
        let u = Url::parse(url.trim())?;
        if u.path().to_ascii_lowercase().ends_with(".xml") {
            return Ok(u);
        }
        let mut k = Kvp::new();
        k.push("SERVICE", "WMTS")
            .push("REQUEST", "GetCapabilities")
            .push("VERSION", "1.0.0");
        build_url(clean_service_url(url)?.as_str(), &k.0)
    }

    /// Parsed capabilities.
    pub fn capabilities(&self) -> &WmtsCapabilities {
        &self.capabilities
    }

    /// The HTTP client.
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Resolves a request into a tile URL (no network access).
    pub fn get_tile_url(&self, req: &GetTile) -> Result<Url> {
        let layer = self
            .capabilities
            .layer(&req.layer)
            .ok_or_else(|| Error::InvalidRequest(format!("unknown layer: {}", req.layer)))?;
        if !layer.tile_matrix_set_links.is_empty()
            && !layer.has_tile_matrix_set(&req.tile_matrix_set)
        {
            return Err(Error::InvalidRequest(format!(
                "layer {} is not available in tile matrix set {}",
                req.layer, req.tile_matrix_set
            )));
        }
        let style = req
            .style
            .clone()
            .or_else(|| layer.default_style().map(str::to_string))
            .unwrap_or_else(|| "default".into());
        let format = req
            .format
            .clone()
            .or_else(|| layer.formats.first().cloned())
            .or_else(|| {
                layer
                    .resource_urls
                    .iter()
                    .find(|r| r.resource_type == "tile")
                    .map(|r| r.format.clone())
            })
            .unwrap_or_else(|| "image/png".into());
        // Dimension values: explicit, else defaults.
        let mut dims: Vec<(String, String)> = Vec::new();
        for d in &layer.dimensions {
            let v = req
                .dimensions
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(&d.identifier))
                .map(|(_, v)| v.clone())
                .or_else(|| d.default.clone());
            if let Some(v) = v {
                dims.push((d.identifier.clone(), v));
            }
        }
        for (k, v) in &req.dimensions {
            if !dims.iter().any(|(d, _)| d.eq_ignore_ascii_case(k)) {
                dims.push((k.clone(), v.clone()));
            }
        }

        let kvp_url = self
            .capabilities
            .operation("GetTile")
            .and_then(|o| o.get_url_for_encoding("KVP"));
        let use_rest = match req.encoding {
            TileEncoding::Rest => true,
            TileEncoding::Kvp => false,
            TileEncoding::Auto => layer
                .resource_urls
                .iter()
                .any(|r| r.resource_type == "tile"),
        };

        if use_rest {
            let mut values = vec![
                ("Style".to_string(), style.clone()),
                ("TileMatrixSet".to_string(), req.tile_matrix_set.clone()),
                ("TileMatrix".to_string(), req.tile_matrix.clone()),
                ("TileRow".to_string(), req.row.to_string()),
                ("TileCol".to_string(), req.col.to_string()),
            ];
            values.extend(dims.iter().cloned());
            let mut candidates: Vec<&ResourceUrl> = layer
                .resource_urls
                .iter()
                .filter(|r| r.resource_type == "tile" && r.format == format)
                .collect();
            if candidates.is_empty() {
                candidates = layer
                    .resource_urls
                    .iter()
                    .filter(|r| r.resource_type == "tile")
                    .collect();
            }
            // Prefer the template that uses the most of the supplied values.
            candidates.sort_by_key(|r| std::cmp::Reverse(r.template.matches('{').count()));
            for c in candidates {
                if let Some(u) = fill_template(&c.template, &values) {
                    return Ok(Url::parse(&u)?);
                }
            }
            if req.encoding == TileEncoding::Rest {
                return Err(Error::Unsupported(format!(
                    "no usable ResourceURL template for layer {}",
                    req.layer
                )));
            }
        }

        let base = kvp_url
            .map(str::to_string)
            .unwrap_or_else(|| self.url.to_string());
        let mut k = Kvp::new();
        k.push("SERVICE", "WMTS")
            .push("REQUEST", "GetTile")
            .push("VERSION", "1.0.0")
            .push("LAYER", req.layer.clone())
            .push("STYLE", style)
            .push("FORMAT", format)
            .push("TILEMATRIXSET", req.tile_matrix_set.clone())
            .push("TILEMATRIX", req.tile_matrix.clone())
            .push("TILEROW", req.row.to_string())
            .push("TILECOL", req.col.to_string());
        for (d, v) in dims {
            k.push(&d.to_ascii_uppercase(), v);
        }
        build_url(&base, &k.0)
    }

    /// Downloads a tile.
    pub async fn get_tile(&self, req: &GetTile) -> Result<WmsResponse> {
        let url = self.get_tile_url(req)?;
        WmsResponse::from_http(self.client.get(&url, None).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates() {
        let v = vec![
            ("TileMatrix".to_string(), "3".to_string()),
            ("time".to_string(), "2024".to_string()),
        ];
        assert_eq!(
            fill_template("https://x/{Time}/{tilematrix}.png", &v).as_deref(),
            Some("https://x/2024/3.png")
        );
        assert!(fill_template("https://x/{TileRow}", &v).is_none());
    }
}
