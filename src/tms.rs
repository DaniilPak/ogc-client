//! Tile matrix set math shared by WMTS 1.0.0 and OGC API – Tiles
//! (OGC 17-083r4 "Two Dimensional Tile Matrix Set").
//!
//! All coordinates in this module are x/y (easting/longitude first).

use serde::{Deserialize, Serialize};

use crate::crs::{AxisOrder, Crs};

/// Standardized rendering pixel size (0.28 mm) used by OGC scale denominators.
pub const STANDARD_PIXEL_SIZE: f64 = 0.28e-3;

/// One zoom level of a tile matrix set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TileMatrix {
    /// Identifier used in requests (`TILEMATRIX`, `{tileMatrix}`).
    pub identifier: String,
    /// Scale denominator.
    pub scale_denominator: f64,
    /// Top-left corner in x/y order (already normalized from the CRS axis order).
    pub top_left: (f64, f64),
    /// Tile width in pixels.
    pub tile_width: u32,
    /// Tile height in pixels.
    pub tile_height: u32,
    /// Number of tile columns.
    pub matrix_width: u64,
    /// Number of tile rows.
    pub matrix_height: u64,
}

/// A tile address within a tile matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TileIndex {
    /// Row (from the top).
    pub row: u64,
    /// Column (from the left).
    pub col: u64,
}

impl TileMatrix {
    /// Size of one pixel in CRS units, given meters per CRS unit.
    pub fn pixel_size(&self, meters_per_unit: f64) -> f64 {
        self.scale_denominator * STANDARD_PIXEL_SIZE / meters_per_unit
    }

    /// Width and height of one tile in CRS units.
    pub fn tile_span(&self, meters_per_unit: f64) -> (f64, f64) {
        let p = self.pixel_size(meters_per_unit);
        (self.tile_width as f64 * p, self.tile_height as f64 * p)
    }

    /// Bounding box `(minx, miny, maxx, maxy)` of a tile.
    pub fn tile_bbox(&self, tile: TileIndex, meters_per_unit: f64) -> (f64, f64, f64, f64) {
        let (sx, sy) = self.tile_span(meters_per_unit);
        let minx = self.top_left.0 + tile.col as f64 * sx;
        let maxy = self.top_left.1 - tile.row as f64 * sy;
        (minx, maxy - sy, minx + sx, maxy)
    }

    /// Extent `(minx, miny, maxx, maxy)` covered by the whole matrix.
    pub fn extent(&self, meters_per_unit: f64) -> (f64, f64, f64, f64) {
        let (sx, sy) = self.tile_span(meters_per_unit);
        (
            self.top_left.0,
            self.top_left.1 - sy * self.matrix_height as f64,
            self.top_left.0 + sx * self.matrix_width as f64,
            self.top_left.1,
        )
    }

    /// Tile containing the point, or `None` if outside the matrix.
    pub fn tile_at(&self, x: f64, y: f64, meters_per_unit: f64) -> Option<TileIndex> {
        let (sx, sy) = self.tile_span(meters_per_unit);
        let col = ((x - self.top_left.0) / sx).floor();
        let row = ((self.top_left.1 - y) / sy).floor();
        if col < 0.0 || row < 0.0 {
            return None;
        }
        let (col, row) = (col as u64, row as u64);
        if col >= self.matrix_width || row >= self.matrix_height {
            return None;
        }
        Some(TileIndex { row, col })
    }

    /// Inclusive `(min, max)` tile ranges covering a bbox, clipped to the matrix.
    /// Returns `None` if the bbox does not intersect the matrix.
    pub fn tile_range(
        &self,
        bbox: (f64, f64, f64, f64),
        meters_per_unit: f64,
    ) -> Option<(TileIndex, TileIndex)> {
        let (sx, sy) = self.tile_span(meters_per_unit);
        let (minx, miny, maxx, maxy) = bbox;
        // A small epsilon so that a bbox ending exactly on a tile edge does
        // not include the next tile.
        let eps = 1e-9;
        let c0 = ((minx - self.top_left.0) / sx + eps).floor();
        let c1 = ((maxx - self.top_left.0) / sx - eps).floor();
        let r0 = ((self.top_left.1 - maxy) / sy + eps).floor();
        let r1 = ((self.top_left.1 - miny) / sy - eps).floor();
        let maxc = self.matrix_width as f64 - 1.0;
        let maxr = self.matrix_height as f64 - 1.0;
        if c1 < 0.0 || r1 < 0.0 || c0 > maxc || r0 > maxr {
            return None;
        }
        Some((
            TileIndex {
                row: r0.max(0.0) as u64,
                col: c0.max(0.0) as u64,
            },
            TileIndex {
                row: r1.min(maxr) as u64,
                col: c1.min(maxc) as u64,
            },
        ))
    }

    /// All tiles covering a bbox (row-major).
    pub fn tiles_in(&self, bbox: (f64, f64, f64, f64), meters_per_unit: f64) -> Vec<TileIndex> {
        match self.tile_range(bbox, meters_per_unit) {
            None => Vec::new(),
            Some((a, b)) => (a.row..=b.row)
                .flat_map(|row| (a.col..=b.col).map(move |col| TileIndex { row, col }))
                .collect(),
        }
    }
}

/// A tile matrix set: a CRS plus a pyramid of [`TileMatrix`] levels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TileMatrixSet {
    /// Identifier.
    pub identifier: String,
    /// Title.
    pub title: Option<String>,
    /// CRS identifier as advertised.
    pub crs: String,
    /// Well-known scale set URI (WMTS) or definition URI (OGC API).
    pub well_known_scale_set: Option<String>,
    /// Bounding box, if declared (x/y order).
    pub bounding_box: Option<(f64, f64, f64, f64)>,
    /// Levels, from coarse to fine.
    pub tile_matrices: Vec<TileMatrix>,
}

impl TileMatrixSet {
    /// Parsed CRS.
    pub fn crs(&self) -> Option<Crs> {
        Crs::parse(&self.crs)
    }

    /// Meters per CRS unit (degree length for geographic CRSs).
    pub fn meters_per_unit(&self) -> f64 {
        self.crs().map(|c| c.meters_per_unit()).unwrap_or(1.0)
    }

    /// Level by identifier.
    pub fn matrix(&self, id: &str) -> Option<&TileMatrix> {
        self.tile_matrices.iter().find(|m| m.identifier == id)
    }

    /// Bounding box of a tile in a given level.
    pub fn tile_bbox(&self, matrix: &str, tile: TileIndex) -> Option<(f64, f64, f64, f64)> {
        Some(self.matrix(matrix)?.tile_bbox(tile, self.meters_per_unit()))
    }

    /// Tile containing a point.
    pub fn tile_at(&self, matrix: &str, x: f64, y: f64) -> Option<TileIndex> {
        self.matrix(matrix)?.tile_at(x, y, self.meters_per_unit())
    }

    /// Level whose resolution is closest to `units_per_pixel`.
    pub fn matrix_for_resolution(&self, units_per_pixel: f64) -> Option<&TileMatrix> {
        let mpu = self.meters_per_unit();
        self.tile_matrices.iter().min_by(|a, b| {
            let da = (a.pixel_size(mpu) - units_per_pixel).abs();
            let db = (b.pixel_size(mpu) - units_per_pixel).abs();
            da.total_cmp(&db)
        })
    }
}

/// Converts a corner given in CRS axis order into x/y.
///
/// Some servers (NASA GIBS among them) write x/y even for y/x CRSs; for
/// geographic CRSs a first value outside ±90 is taken as a longitude.
pub(crate) fn normalize_corner(crs: Option<&Crs>, a: f64, b: f64) -> (f64, f64) {
    match crs {
        Some(c) if c.authority_axis_order() == AxisOrder::YX => {
            if c.is_geographic() && a.abs() > 90.0 {
                (a, b)
            } else {
                (b, a)
            }
        }
        _ => (a, b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn web_mercator_z(z: u32) -> TileMatrix {
        let half = 20037508.342789244;
        TileMatrix {
            identifier: z.to_string(),
            scale_denominator: 559082264.0287178 / 2f64.powi(z as i32),
            top_left: (-half, half),
            tile_width: 256,
            tile_height: 256,
            matrix_width: 1 << z,
            matrix_height: 1 << z,
        }
    }

    #[test]
    fn web_mercator_math() {
        let m = web_mercator_z(1);
        let (sx, _) = m.tile_span(1.0);
        assert!((sx - 20037508.342789244).abs() < 1e-3);
        assert_eq!(m.tile_at(1.0, 1.0, 1.0), Some(TileIndex { row: 0, col: 1 }));
        assert_eq!(
            m.tile_at(-1.0, -1.0, 1.0),
            Some(TileIndex { row: 1, col: 0 })
        );
        let b = m.tile_bbox(TileIndex { row: 1, col: 1 }, 1.0);
        assert!(b.0.abs() < 1e-6 && b.3.abs() < 1e-6);
        let z3 = web_mercator_z(3);
        assert_eq!(z3.tiles_in(z3.extent(1.0), 1.0).len(), 64);
        // bbox exactly on tile edges covers just one tile
        let t = z3.tile_bbox(TileIndex { row: 2, col: 5 }, 1.0);
        assert_eq!(z3.tiles_in(t, 1.0), vec![TileIndex { row: 2, col: 5 }]);
        assert!(z3.tile_range((3e7, 3e7, 4e7, 4e7), 1.0).is_none());
    }

    #[test]
    fn corner_normalization() {
        let c = Crs::parse("urn:ogc:def:crs:EPSG::4326").unwrap();
        assert_eq!(normalize_corner(Some(&c), 90.0, -180.0), (-180.0, 90.0));
        assert_eq!(normalize_corner(Some(&c), -180.0, 90.0), (-180.0, 90.0));
        let m = Crs::epsg(3857);
        assert_eq!(normalize_corner(Some(&m), 1.0, 2.0), (1.0, 2.0));
    }
}
