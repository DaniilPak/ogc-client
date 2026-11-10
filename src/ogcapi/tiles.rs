//! OGC API – Tiles (Part 1) and the JSON Tile Matrix Set encoding (OGC 17-083r4).
//!
//! ```no_run
//! # async fn run() -> ogc_client::Result<()> {
//! use ogc_client::ogcapi::Tiles;
//!
//! let api = Tiles::new("https://demo.pygeoapi.io/master")?;
//! let sets = api.collection_tilesets("lakes").await?;
//! let ts = api.tileset_from_summary(&sets.tilesets[0]).await?;
//! let tms = api.tile_matrix_set(&ts).await?.to_tile_matrix_set();
//! let idx = tms.tile_at("3", 2.35e5, 6.25e6).unwrap(); // Paris in EPSG:3857
//! let mvt = api.get_tile(&ts, "3", idx.row, idx.col).await?;
//! println!("{} bytes", mvt.body.len());
//! # Ok(()) }
//! ```

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use url::Url;

use super::{Api, Link, LinkExt, deref_api};
use crate::crs::{AxisOrder, Crs};
use crate::error::{Error, Result};
use crate::http::Client;
use crate::tms::{TileMatrix, TileMatrixSet};
use crate::wms::WmsResponse;

/// CRS can be a string or `{ "uri": ... }` / `{ "wkt": ... }`.
fn lenient_crs<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let v = Option::<Value>::deserialize(d)?;
    Ok(match v {
        Some(Value::String(s)) => Some(s),
        Some(Value::Object(o)) => o
            .get("uri")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| Some(Value::Object(o).to_string())),
        _ => None,
    })
}

/// Entry of a tileset list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TileSetSummary {
    /// Title.
    #[serde(default)]
    pub title: Option<String>,
    /// `vector`, `map` or `coverage`.
    #[serde(rename = "dataType", default)]
    pub data_type: Option<String>,
    /// CRS.
    #[serde(default, deserialize_with = "lenient_crs")]
    pub crs: Option<String>,
    /// URI of the tile matrix set.
    #[serde(rename = "tileMatrixSetURI", default)]
    pub tile_matrix_set_uri: Option<String>,
    /// Links (`self` points at the tileset metadata).
    #[serde(default)]
    pub links: Vec<Link>,
    /// Other members.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl TileSetSummary {
    /// Tile matrix set id, taken from the URI's last segment.
    pub fn tile_matrix_set_id(&self) -> Option<&str> {
        self.tile_matrix_set_uri
            .as_deref()
            .and_then(|u| u.trim_end_matches('/').rsplit('/').next())
    }
}

/// Tileset list (`/tiles`, `/collections/{id}/tiles`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TileSetList {
    /// Tilesets.
    #[serde(default)]
    pub tilesets: Vec<TileSetSummary>,
    /// Links.
    #[serde(default)]
    pub links: Vec<Link>,
}

/// Limits of one tile matrix in a tileset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TileMatrixSetLimit {
    /// Tile matrix id.
    #[serde(rename = "tileMatrix")]
    pub tile_matrix: String,
    /// Min row.
    #[serde(rename = "minTileRow")]
    pub min_tile_row: u64,
    /// Max row.
    #[serde(rename = "maxTileRow")]
    pub max_tile_row: u64,
    /// Min column.
    #[serde(rename = "minTileCol")]
    pub min_tile_col: u64,
    /// Max column.
    #[serde(rename = "maxTileCol")]
    pub max_tile_col: u64,
}

/// Tileset metadata (`/collections/{id}/tiles/{tileMatrixSetId}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TileSet {
    /// Title.
    #[serde(default)]
    pub title: Option<String>,
    /// Description.
    #[serde(default)]
    pub description: Option<String>,
    /// `vector`, `map` or `coverage`.
    #[serde(rename = "dataType", default)]
    pub data_type: Option<String>,
    /// CRS.
    #[serde(default, deserialize_with = "lenient_crs")]
    pub crs: Option<String>,
    /// Tile matrix set URI.
    #[serde(rename = "tileMatrixSetURI", default)]
    pub tile_matrix_set_uri: Option<String>,
    /// Per-level limits.
    #[serde(rename = "tileMatrixSetLimits", default)]
    pub tile_matrix_set_limits: Vec<TileMatrixSetLimit>,
    /// Links (`item` holds the tile URL template, `tiling-scheme` the TMS).
    #[serde(default)]
    pub links: Vec<Link>,
    /// Other members.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl TileSet {
    /// Tile URL templates (`rel=item`), with their media types.
    pub fn tile_templates(&self) -> Vec<&Link> {
        self.links.all_rel("item")
    }

    /// Fills the template for a tile. `format` selects a template by media type.
    pub fn tile_url(&self, matrix: &str, row: u64, col: u64, format: Option<&str>) -> Result<Url> {
        let templates = self.tile_templates();
        let link = match format {
            Some(f) => templates
                .iter()
                .find(|l| l.type_.as_deref() == Some(f))
                .copied(),
            None => templates.first().copied(),
        }
        .ok_or_else(|| Error::Unsupported("tileset has no tile URL template".into()))?;
        let href = link
            .href
            .replace("{tileMatrix}", matrix)
            .replace("{tileRow}", &row.to_string())
            .replace("{tileCol}", &col.to_string());
        Ok(Url::parse(&href)?)
    }

    /// Whether a tile is inside the declared limits (true if no limits).
    pub fn contains_tile(&self, matrix: &str, row: u64, col: u64) -> bool {
        match self
            .tile_matrix_set_limits
            .iter()
            .find(|l| l.tile_matrix == matrix)
        {
            Some(l) => {
                (l.min_tile_row..=l.max_tile_row).contains(&row)
                    && (l.min_tile_col..=l.max_tile_col).contains(&col)
            }
            None => self.tile_matrix_set_limits.is_empty(),
        }
    }
}

/// One level of a JSON tile matrix set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TileMatrixDef {
    /// Identifier.
    pub id: String,
    /// Scale denominator.
    #[serde(rename = "scaleDenominator")]
    pub scale_denominator: f64,
    /// Cell size in CRS units.
    #[serde(rename = "cellSize", default)]
    pub cell_size: Option<f64>,
    /// `topLeft` (default) or `bottomLeft`.
    #[serde(rename = "cornerOfOrigin", default)]
    pub corner_of_origin: Option<String>,
    /// Origin in the order given by `orderedAxes`.
    #[serde(rename = "pointOfOrigin")]
    pub point_of_origin: Vec<f64>,
    /// Tile width.
    #[serde(rename = "tileWidth")]
    pub tile_width: u32,
    /// Tile height.
    #[serde(rename = "tileHeight")]
    pub tile_height: u32,
    /// Columns.
    #[serde(rename = "matrixWidth")]
    pub matrix_width: u64,
    /// Rows.
    #[serde(rename = "matrixHeight")]
    pub matrix_height: u64,
}

/// A JSON tile matrix set definition (`/tileMatrixSets/{id}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TileMatrixSetDef {
    /// Identifier.
    #[serde(default)]
    pub id: Option<String>,
    /// Title.
    #[serde(default)]
    pub title: Option<String>,
    /// Definition URI.
    #[serde(default)]
    pub uri: Option<String>,
    /// CRS.
    #[serde(default, deserialize_with = "lenient_crs")]
    pub crs: Option<String>,
    /// Axis labels in coordinate order (e.g. `["X","Y"]`, `["Lat","Lon"]`).
    #[serde(rename = "orderedAxes", default)]
    pub ordered_axes: Vec<String>,
    /// Well-known scale set.
    #[serde(rename = "wellKnownScaleSet", default)]
    pub well_known_scale_set: Option<String>,
    /// Levels.
    #[serde(rename = "tileMatrices", default)]
    pub tile_matrices: Vec<TileMatrixDef>,
}

impl TileMatrixSetDef {
    fn origin_is_yx(&self) -> bool {
        match self.ordered_axes.first() {
            Some(a) => {
                let a = a.to_ascii_lowercase();
                a.starts_with("lat") || a == "y" || a == "n" || a.starts_with("north")
            }
            None => self
                .crs
                .as_deref()
                .and_then(Crs::parse)
                .map(|c| c.authority_axis_order() == AxisOrder::YX)
                .unwrap_or(false),
        }
    }

    /// Converts to the crate's [`TileMatrixSet`] (x/y, top-left origin) so
    /// the same tile math as WMTS applies.
    pub fn to_tile_matrix_set(&self) -> TileMatrixSet {
        let crs = self.crs.clone().unwrap_or_default();
        let mpu = Crs::parse(&crs).map(|c| c.meters_per_unit()).unwrap_or(1.0);
        let yx = self.origin_is_yx();
        let tile_matrices = self
            .tile_matrices
            .iter()
            .filter(|m| m.point_of_origin.len() >= 2)
            .map(|m| {
                let (mut x, mut y) = if yx {
                    (m.point_of_origin[1], m.point_of_origin[0])
                } else {
                    (m.point_of_origin[0], m.point_of_origin[1])
                };
                // Use the scale denominator to stay consistent with TileMatrix::pixel_size.
                let cell = m
                    .cell_size
                    .unwrap_or(m.scale_denominator * crate::tms::STANDARD_PIXEL_SIZE / mpu);
                if m.corner_of_origin.as_deref() == Some("bottomLeft") {
                    y += cell * m.tile_height as f64 * m.matrix_height as f64;
                }
                if x.is_nan() {
                    x = 0.0;
                }
                TileMatrix {
                    identifier: m.id.clone(),
                    scale_denominator: m.scale_denominator,
                    top_left: (x, y),
                    tile_width: m.tile_width,
                    tile_height: m.tile_height,
                    matrix_width: m.matrix_width,
                    matrix_height: m.matrix_height,
                }
            })
            .collect();
        TileMatrixSet {
            identifier: self.id.clone().unwrap_or_default(),
            title: self.title.clone(),
            crs,
            well_known_scale_set: self
                .well_known_scale_set
                .clone()
                .or_else(|| self.uri.clone()),
            bounding_box: None,
            tile_matrices,
        }
    }
}

/// Entry of `/tileMatrixSets`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TileMatrixSetRef {
    /// Identifier.
    #[serde(default)]
    pub id: Option<String>,
    /// Title.
    #[serde(default)]
    pub title: Option<String>,
    /// URI.
    #[serde(default)]
    pub uri: Option<String>,
    /// Links.
    #[serde(default)]
    pub links: Vec<Link>,
}

/// `/tileMatrixSets` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TileMatrixSetList {
    /// Tile matrix sets.
    #[serde(rename = "tileMatrixSets", default)]
    pub tile_matrix_sets: Vec<TileMatrixSetRef>,
}

/// OGC API – Tiles client.
#[derive(Debug, Clone)]
pub struct Tiles {
    pub(crate) api: Api,
}

deref_api!(Tiles);

impl Tiles {
    /// A client for the API rooted at `url`.
    pub fn new(url: &str) -> Result<Tiles> {
        Ok(Tiles {
            api: Api::new(url)?,
        })
    }

    /// With a custom HTTP client.
    pub fn with_client(client: Client, url: &str) -> Result<Tiles> {
        Ok(Tiles {
            api: Api::with_client(client, url)?,
        })
    }

    /// Dataset tilesets (`/tiles`).
    pub async fn tilesets(&self) -> Result<TileSetList> {
        self.api.get_json(&["tiles"], &[]).await
    }

    /// Collection tilesets (`/collections/{id}/tiles`).
    pub async fn collection_tilesets(&self, collection: &str) -> Result<TileSetList> {
        self.api
            .get_json(&["collections", collection, "tiles"], &[])
            .await
    }

    /// Tileset metadata for a collection and tile matrix set.
    pub async fn collection_tileset(&self, collection: &str, tms: &str) -> Result<TileSet> {
        self.api
            .get_json(&["collections", collection, "tiles", tms], &[])
            .await
    }

    /// Follows the `self` link of a tileset summary.
    pub async fn tileset_from_summary(&self, s: &TileSetSummary) -> Result<TileSet> {
        let link = s
            .links
            .find_rel("self")
            .ok_or_else(|| Error::Parse("tileset summary has no self link".into()))?;
        self.api
            .get_json_url(&Url::parse(&link.href)?, "application/json")
            .await
    }

    /// Available tile matrix sets.
    pub async fn tile_matrix_sets(&self) -> Result<TileMatrixSetList> {
        self.api.get_json(&["tileMatrixSets"], &[]).await
    }

    /// A tile matrix set definition by id.
    pub async fn tile_matrix_set_by_id(&self, id: &str) -> Result<TileMatrixSetDef> {
        self.api.get_json(&["tileMatrixSets", id], &[]).await
    }

    /// The tile matrix set of a tileset (via its `tiling-scheme` link, else by id).
    pub async fn tile_matrix_set(&self, ts: &TileSet) -> Result<TileMatrixSetDef> {
        if let Some(l) = ts.links.find_rel("tiling-scheme") {
            return self
                .api
                .get_json_url(&Url::parse(&l.href)?, "application/json")
                .await;
        }
        let id = ts
            .tile_matrix_set_uri
            .as_deref()
            .and_then(|u| u.rsplit('/').next())
            .ok_or_else(|| Error::Parse("tileset does not name its tile matrix set".into()))?;
        self.tile_matrix_set_by_id(id).await
    }

    /// Downloads a tile.
    pub async fn get_tile(
        &self,
        ts: &TileSet,
        matrix: &str,
        row: u64,
        col: u64,
    ) -> Result<WmsResponse> {
        if !ts.contains_tile(matrix, row, col) {
            return Err(Error::InvalidRequest(format!(
                "tile {matrix}/{row}/{col} is outside the tileset limits"
            )));
        }
        let url = ts.tile_url(matrix, row, col, None)?;
        WmsResponse::from_http(self.api.client().get(&url, None).await?)
    }
}
