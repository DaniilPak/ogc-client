//! Reading GML 2 / 3.1 / 3.2 feature collections into `geo-types` geometries
//! and GeoJSON.
//!
//! Coordinates are returned in x/y (longitude/easting first) order. For
//! `urn:`/`http://www.opengis.net/def/crs/` srsNames the EPSG axis order is
//! applied. For the ambiguous short `EPSG:4326` form, servers disagree
//! (GeoServer writes lon/lat, MapServer WFS 1.1 writes lat/lon), so the
//! order is detected from the coordinate ranges of the whole collection.

use geo_types::{
    Coord, Geometry, GeometryCollection, LineString, MultiLineString, MultiPoint, MultiPolygon,
    Point, Polygon, Rect,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::crs::{AxisOrder, Crs, CrsForm};
use crate::error::{Error, Result, parse_xml_checked};
use crate::xml::{Element, parse_numbers};

/// GML namespaces (2/3.1 and 3.2).
const GML_NS: [&str; 2] = [
    "http://www.opengis.net/gml",
    "http://www.opengis.net/gml/3.2",
];

fn is_gml(e: &Element) -> bool {
    e.namespace
        .as_deref()
        .map(|n| GML_NS.contains(&n))
        .unwrap_or(false)
}

const GEOMETRY_NAMES: &[&str] = &[
    "Point",
    "LineString",
    "LinearRing",
    "Polygon",
    "Envelope",
    "Box",
    "MultiPoint",
    "MultiLineString",
    "MultiCurve",
    "MultiPolygon",
    "MultiSurface",
    "MultiGeometry",
    "Curve",
    "Surface",
    "CompositeCurve",
    "CompositeSurface",
    "OrientableSurface",
];

/// Whether an element is a GML geometry.
pub fn is_geometry(e: &Element) -> bool {
    is_gml(e) && GEOMETRY_NAMES.contains(&e.name.as_str())
}

/// A feature read from GML.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GmlFeature {
    /// `gml:id` / `fid`.
    pub id: Option<String>,
    /// Local name of the feature type element (e.g. `cities`).
    pub type_name: String,
    /// Namespace of the feature type.
    pub namespace: Option<String>,
    /// Name of the property holding [`GmlFeature::geometry`].
    pub geometry_property: Option<String>,
    /// First geometry of the feature, x/y order.
    pub geometry: Option<Geometry<f64>>,
    /// srsName of the geometry.
    pub srs_name: Option<String>,
    /// Non-geometry properties. Numbers and booleans are typed, nested
    /// elements become objects; extra geometries become GeoJSON objects.
    pub properties: Map<String, Value>,
}

/// A feature collection read from GML.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GmlFeatureCollection {
    /// Features.
    pub features: Vec<GmlFeature>,
    /// `numberMatched` (WFS 2.0) or `numberOfFeatures` (WFS 1.1); `None` if `unknown`.
    pub number_matched: Option<u64>,
    /// `numberReturned` (WFS 2.0).
    pub number_returned: Option<u64>,
    /// `next` link (WFS 2.0 paging).
    pub next: Option<String>,
    /// `previous` link (WFS 2.0 paging).
    pub previous: Option<String>,
    /// `timeStamp`.
    pub timestamp: Option<String>,
}

impl GmlFeatureCollection {
    /// Parses a `wfs:FeatureCollection` / `gml:FeatureCollection` document.
    pub fn parse(bytes: &[u8]) -> Result<GmlFeatureCollection> {
        let root = parse_xml_checked(bytes)?;
        Self::from_element(&root)
    }

    /// Reads from an already parsed root element.
    pub fn from_element(root: &Element) -> Result<GmlFeatureCollection> {
        if !root.name.ends_with("FeatureCollection") {
            return Err(Error::Parse(format!(
                "expected a FeatureCollection, found <{}>",
                root.name
            )));
        }
        let num = |a: &str| root.attr(a).and_then(|v| v.trim().parse::<u64>().ok());
        let mut raw = Vec::new();
        collect_members(root, &mut raw);
        let ambiguous_swap = decide_short_axis(&raw);
        let features = raw.into_iter().map(|r| r.finish(ambiguous_swap)).collect();
        Ok(GmlFeatureCollection {
            features,
            number_matched: num("numberMatched").or_else(|| num("numberOfFeatures")),
            number_returned: num("numberReturned").or_else(|| num("numberOfFeatures")),
            next: root.attr("next").map(str::to_string),
            previous: root.attr("previous").map(str::to_string),
            timestamp: root
                .attr("timeStamp")
                .or_else(|| root.attr("timestamp"))
                .map(str::to_string),
        })
    }

    /// Converts to a GeoJSON feature collection.
    pub fn to_geojson(&self) -> geojson::FeatureCollection {
        let features = self.features.iter().map(GmlFeature::to_geojson).collect();
        let mut fc = geojson::FeatureCollection::new(Vec::<geojson::Feature>::new());
        fc.features = features;
        let mut fm = Map::new();
        if let Some(n) = self.number_matched {
            fm.insert("numberMatched".into(), n.into());
        }
        if let Some(n) = self.number_returned {
            fm.insert("numberReturned".into(), n.into());
        }
        if !fm.is_empty() {
            fc.foreign_members = Some(fm);
        }
        fc
    }
}

impl GmlFeature {
    /// Converts to a GeoJSON feature.
    pub fn to_geojson(&self) -> geojson::Feature {
        geojson::Feature {
            bbox: None,
            geometry: self
                .geometry
                .as_ref()
                .map(|g| geojson::Geometry::new(geojson::GeometryValue::from(g))),
            id: self.id.clone().map(geojson::feature::Id::String),
            properties: Some(self.properties.clone()),
            foreign_members: None,
        }
    }
}

/// Raw geometry before axis-order resolution.
struct RawGeom {
    geom: Geometry<f64>,
    srs: Option<String>,
}

enum AxisDecision {
    Swap,
    Keep,
    Ambiguous,
}

impl RawGeom {
    fn decision(&self) -> AxisDecision {
        match self.srs.as_deref().and_then(Crs::parse) {
            None => AxisDecision::Keep,
            Some(c) => match c.form {
                CrsForm::Urn | CrsForm::Uri => {
                    if c.authority_axis_order() == AxisOrder::YX {
                        AxisDecision::Swap
                    } else {
                        AxisDecision::Keep
                    }
                }
                CrsForm::GmlSrs => AxisDecision::Keep,
                CrsForm::Short => {
                    if c.authority_axis_order() == AxisOrder::YX && c.is_geographic() {
                        AxisDecision::Ambiguous
                    } else {
                        AxisDecision::Keep
                    }
                }
            },
        }
    }
}

struct RawFeature {
    id: Option<String>,
    type_name: String,
    namespace: Option<String>,
    geometry_property: Option<String>,
    geometry: Option<RawGeom>,
    properties: Map<String, Value>,
    extra_geoms: Vec<(String, RawGeom)>,
}

impl RawFeature {
    fn finish(self, ambiguous_swap: bool) -> GmlFeature {
        let fix = |g: RawGeom| -> (Geometry<f64>, Option<String>) {
            let swap = match g.decision() {
                AxisDecision::Swap => true,
                AxisDecision::Keep => false,
                AxisDecision::Ambiguous => ambiguous_swap,
            };
            let geom = if swap { swap_xy(g.geom) } else { g.geom };
            (geom, g.srs)
        };
        let mut properties = self.properties;
        for (name, g) in self.extra_geoms {
            let (geom, _) = fix(g);
            if let Ok(v) =
                serde_json::to_value(geojson::Geometry::new(geojson::GeometryValue::from(&geom)))
            {
                properties.insert(name, v);
            }
        }
        let (geometry, srs_name) = match self.geometry.map(fix) {
            Some((g, s)) => (Some(g), s),
            None => (None, None),
        };
        GmlFeature {
            id: self.id,
            type_name: self.type_name,
            namespace: self.namespace,
            geometry_property: self.geometry_property,
            geometry,
            srs_name,
            properties,
        }
    }
}

/// Decides whether ambiguous short-form geographic coordinates are lat/lon.
fn decide_short_axis(raw: &[RawFeature]) -> bool {
    let mut first_out = false; // some first value outside ±90 → first is lon
    let mut second_out = false; // some second value outside ±90 → second is lon
    for f in raw {
        let geoms = f
            .geometry
            .iter()
            .chain(f.extra_geoms.iter().map(|(_, g)| g));
        for g in geoms {
            if !matches!(g.decision(), AxisDecision::Ambiguous) {
                continue;
            }
            for c in coords(&g.geom) {
                if c.x.abs() > 90.0 {
                    first_out = true;
                }
                if c.y.abs() > 90.0 {
                    second_out = true;
                }
            }
        }
    }
    // Lat/lon only if the data proves it; otherwise keep the x/y convention.
    second_out && !first_out
}

fn coords(g: &Geometry<f64>) -> Vec<Coord<f64>> {
    let mut out = Vec::new();
    walk_coords(g, &mut |c| out.push(*c));
    out
}

fn walk_coords(g: &Geometry<f64>, f: &mut dyn FnMut(&Coord<f64>)) {
    let ring = |l: &LineString<f64>, f: &mut dyn FnMut(&Coord<f64>)| l.0.iter().for_each(f);
    let poly = |p: &Polygon<f64>, f: &mut dyn FnMut(&Coord<f64>)| {
        ring(p.exterior(), f);
        p.interiors().iter().for_each(|r| ring(r, f));
    };
    match g {
        Geometry::Point(p) => f(&p.0),
        Geometry::Line(l) => {
            f(&l.start);
            f(&l.end);
        }
        Geometry::LineString(l) => ring(l, f),
        Geometry::Polygon(p) => poly(p, f),
        Geometry::MultiPoint(m) => m.0.iter().for_each(|p| f(&p.0)),
        Geometry::MultiLineString(m) => m.0.iter().for_each(|l| ring(l, f)),
        Geometry::MultiPolygon(m) => m.0.iter().for_each(|p| poly(p, f)),
        Geometry::GeometryCollection(gc) => gc.0.iter().for_each(|g| walk_coords(g, f)),
        Geometry::Rect(r) => {
            f(&r.min());
            f(&r.max());
        }
        Geometry::Triangle(t) => {
            f(&t.v1());
            f(&t.v2());
            f(&t.v3());
        }
    }
}

fn swap_xy(g: Geometry<f64>) -> Geometry<f64> {
    let sc = |c: Coord<f64>| Coord { x: c.y, y: c.x };
    let sl = |l: LineString<f64>| LineString(l.0.into_iter().map(sc).collect());
    let sp = |p: Polygon<f64>| {
        let (ext, ints) = p.into_inner();
        Polygon::new(sl(ext), ints.into_iter().map(sl).collect())
    };
    match g {
        Geometry::Point(p) => Geometry::Point(Point(sc(p.0))),
        Geometry::Line(l) => Geometry::Line(geo_types::Line::new(sc(l.start), sc(l.end))),
        Geometry::LineString(l) => Geometry::LineString(sl(l)),
        Geometry::Polygon(p) => Geometry::Polygon(sp(p)),
        Geometry::MultiPoint(m) => Geometry::MultiPoint(MultiPoint(
            m.0.into_iter().map(|p| Point(sc(p.0))).collect(),
        )),
        Geometry::MultiLineString(m) => {
            Geometry::MultiLineString(MultiLineString(m.0.into_iter().map(sl).collect()))
        }
        Geometry::MultiPolygon(m) => {
            Geometry::MultiPolygon(MultiPolygon(m.0.into_iter().map(sp).collect()))
        }
        Geometry::GeometryCollection(gc) => Geometry::GeometryCollection(GeometryCollection(
            gc.0.into_iter().map(swap_xy).collect(),
        )),
        Geometry::Rect(r) => Geometry::Rect(Rect::new(sc(r.min()), sc(r.max()))),
        Geometry::Triangle(t) => {
            Geometry::Triangle(geo_types::Triangle::new(sc(t.v1()), sc(t.v2()), sc(t.v3())))
        }
    }
}

fn collect_members(fc: &Element, out: &mut Vec<RawFeature>) {
    for m in &fc.children {
        match m.name.as_str() {
            "member" | "featureMember" | "featureMembers" | "additionalObjects" => {
                for f in &m.children {
                    if f.name.ends_with("FeatureCollection")
                        || f.is("SimpleFeatureCollection")
                        || f.is("additionalValues")
                        || f.is("Tuple")
                    {
                        collect_members(f, out);
                    } else {
                        out.push(read_feature(f));
                    }
                }
            }
            _ => {}
        }
    }
}

fn read_feature(f: &Element) -> RawFeature {
    let id = f
        .attributes
        .iter()
        .find(|(k, _)| k == "gml:id" || k.ends_with(":id") || k == "fid" || k == "id")
        .map(|(_, v)| v.clone());
    let mut properties = Map::new();
    let mut geometry: Option<RawGeom> = None;
    let mut geometry_property = None;
    let mut extra_geoms = Vec::new();
    for p in &f.children {
        if is_gml(p) && p.is("boundedBy") {
            continue;
        }
        if let Some(g) = p.children.iter().find(|c| is_geometry(c))
            && let Some(raw) = read_geometry(g, None)
        {
            if geometry.is_none() {
                geometry = Some(raw);
                geometry_property = Some(p.name.clone());
            } else {
                extra_geoms.push((p.name.clone(), raw));
            }
            continue;
        }
        let value = element_value(p);
        match properties.get_mut(&p.name) {
            Some(Value::Array(arr)) => arr.push(value),
            Some(existing) => {
                let prev = existing.take();
                *existing = Value::Array(vec![prev, value]);
            }
            None => {
                properties.insert(p.name.clone(), value);
            }
        }
    }
    RawFeature {
        id,
        type_name: f.name.clone(),
        namespace: f.namespace.clone(),
        geometry_property,
        geometry,
        properties,
        extra_geoms,
    }
}

/// Converts a property element to JSON: text → typed scalar, children → object.
fn element_value(e: &Element) -> Value {
    if e.attr("nil").map(|v| v == "true").unwrap_or(false) {
        return Value::Null;
    }
    if e.children.is_empty() {
        if let Some(href) = e.attr("href")
            && e.text_trim().is_empty()
        {
            return Value::String(href.to_string());
        }
        return scalar(e.text_trim());
    }
    let mut obj = Map::new();
    for c in &e.children {
        let v = element_value(c);
        match obj.get_mut(&c.name) {
            Some(Value::Array(arr)) => arr.push(v),
            Some(existing) => {
                let prev = existing.take();
                *existing = Value::Array(vec![prev, v]);
            }
            None => {
                obj.insert(c.name.clone(), v);
            }
        }
    }
    Value::Object(obj)
}

fn scalar(s: &str) -> Value {
    if s.is_empty() {
        return Value::Null;
    }
    // Keep values with leading zeros (codes, ZIPs) as strings.
    let leading_zero = s.len() > 1 && s.starts_with('0') && !s.starts_with("0.");
    if !leading_zero {
        if let Ok(i) = s.parse::<i64>() {
            return Value::from(i);
        }
        if s.contains(['.', 'e', 'E'])
            && !s.ends_with('.')
            && let Ok(f) = s.parse::<f64>()
            && f.is_finite()
        {
            return Value::from(f);
        }
    }
    match s {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        _ => Value::String(s.to_string()),
    }
}

/// Reads a GML geometry element into a `geo_types::Geometry` without any
/// axis swapping (coordinates are in document order). Returns `None` for
/// unsupported or empty geometries.
pub fn read_geometry_raw(e: &Element) -> Option<Geometry<f64>> {
    read_geometry(e, None).map(|r| r.geom)
}

fn srs_of(e: &Element, inherited: Option<&str>) -> Option<String> {
    e.attr("srsName")
        .map(str::to_string)
        .or_else(|| inherited.map(str::to_string))
}

fn read_geometry(e: &Element, inherited_srs: Option<&str>) -> Option<RawGeom> {
    let srs = srs_of(e, inherited_srs);
    let dim = e.attr_u32("srsDimension").map(|d| d as usize);
    let geom = match e.name.as_str() {
        "Point" => Geometry::Point(Point(read_coords(e, dim).into_iter().next()?)),
        "LineString" | "LinearRing" => Geometry::LineString(LineString(read_coords(e, dim))),
        "Curve" | "CompositeCurve" => Geometry::LineString(read_curve(e, dim)?),
        "Polygon" => Geometry::Polygon(read_polygon(e, dim)?),
        "Surface" | "CompositeSurface" | "OrientableSurface" => {
            let polys = read_surface(e, dim);
            if polys.len() == 1 {
                Geometry::Polygon(polys.into_iter().next()?)
            } else {
                Geometry::MultiPolygon(MultiPolygon(polys))
            }
        }
        "Envelope" => {
            let lo = parse_numbers(&e.child_text("lowerCorner")?);
            let hi = parse_numbers(&e.child_text("upperCorner")?);
            if lo.len() < 2 || hi.len() < 2 {
                return None;
            }
            Geometry::Rect(Rect::new(
                Coord { x: lo[0], y: lo[1] },
                Coord { x: hi[0], y: hi[1] },
            ))
        }
        "Box" => {
            let c = read_coords(e, dim);
            if c.len() < 2 {
                return None;
            }
            Geometry::Rect(Rect::new(c[0], c[1]))
        }
        "MultiPoint" => {
            let pts = members(e, &["pointMember", "pointMembers"])
                .filter_map(|g| match read_geometry(g, srs.as_deref())?.geom {
                    Geometry::Point(p) => Some(p),
                    _ => None,
                })
                .collect();
            Geometry::MultiPoint(MultiPoint(pts))
        }
        "MultiLineString" | "MultiCurve" => {
            let mut lines = Vec::new();
            for g in members(e, &["lineStringMember", "curveMember", "curveMembers"]) {
                match read_geometry(g, srs.as_deref()).map(|r| r.geom) {
                    Some(Geometry::LineString(l)) => lines.push(l),
                    Some(Geometry::MultiLineString(m)) => lines.extend(m.0),
                    _ => {}
                }
            }
            Geometry::MultiLineString(MultiLineString(lines))
        }
        "MultiPolygon" | "MultiSurface" => {
            let mut polys = Vec::new();
            for g in members(e, &["polygonMember", "surfaceMember", "surfaceMembers"]) {
                match read_geometry(g, srs.as_deref()).map(|r| r.geom) {
                    Some(Geometry::Polygon(p)) => polys.push(p),
                    Some(Geometry::MultiPolygon(m)) => polys.extend(m.0),
                    _ => {}
                }
            }
            Geometry::MultiPolygon(MultiPolygon(polys))
        }
        "MultiGeometry" => {
            let gs = members(e, &["geometryMember", "geometryMembers"])
                .filter_map(|g| read_geometry(g, srs.as_deref()).map(|r| r.geom))
                .collect();
            Geometry::GeometryCollection(GeometryCollection(gs))
        }
        _ => return None,
    };
    Some(RawGeom { geom, srs })
}

/// Geometry children of `*Member` / `*Members` wrappers.
fn members<'a>(e: &'a Element, wrappers: &'a [&'a str]) -> impl Iterator<Item = &'a Element> + 'a {
    e.children
        .iter()
        .filter(move |c| wrappers.contains(&c.name.as_str()))
        .flat_map(|m| m.children.iter().filter(|c| is_geometry(c)))
}

fn read_curve(e: &Element, dim: Option<usize>) -> Option<LineString<f64>> {
    let mut coords: Vec<Coord<f64>> = Vec::new();
    if let Some(segments) = e.child("segments") {
        for seg in &segments.children {
            let c = read_coords(seg, dim);
            append_dedup(&mut coords, c);
        }
    }
    // CompositeCurve: curveMember children.
    for m in e.children_named("curveMember") {
        for g in m.children.iter().filter(|c| is_geometry(c)) {
            if let Some(Geometry::LineString(l)) = read_geometry(g, None).map(|r| r.geom) {
                append_dedup(&mut coords, l.0);
            }
        }
    }
    if coords.is_empty() {
        None
    } else {
        Some(LineString(coords))
    }
}

fn append_dedup(acc: &mut Vec<Coord<f64>>, next: Vec<Coord<f64>>) {
    let mut it = next.into_iter().peekable();
    if let (Some(last), Some(first)) = (acc.last(), it.peek())
        && last == first
    {
        it.next();
    }
    acc.extend(it);
}

fn read_ring(boundary: &Element, dim: Option<usize>) -> Option<LineString<f64>> {
    let ring = boundary
        .children
        .iter()
        .find(|c| c.is("LinearRing") || c.is("Ring"))?;
    if ring.is("Ring") {
        let mut coords = Vec::new();
        for m in ring.children_named("curveMember") {
            for g in &m.children {
                let l = if g.is("LineString") {
                    Some(LineString(read_coords(g, dim)))
                } else {
                    read_curve(g, dim)
                };
                if let Some(l) = l {
                    append_dedup(&mut coords, l.0);
                }
            }
        }
        return Some(LineString(coords));
    }
    Some(LineString(read_coords(ring, dim)))
}

fn read_polygon(e: &Element, dim: Option<usize>) -> Option<Polygon<f64>> {
    let ext = e
        .child_any(&["exterior", "outerBoundaryIs"])
        .and_then(|b| read_ring(b, dim))?;
    let interiors = e
        .children
        .iter()
        .filter(|c| c.is("interior") || c.is("innerBoundaryIs"))
        .filter_map(|b| read_ring(b, dim))
        .collect();
    Some(Polygon::new(ext, interiors))
}

fn read_surface(e: &Element, dim: Option<usize>) -> Vec<Polygon<f64>> {
    let mut out = Vec::new();
    if let Some(patches) = e.child_any(&["patches", "polygonPatches"]) {
        for p in &patches.children {
            if let Some(poly) = read_polygon(p, dim) {
                out.push(poly);
            }
        }
    }
    for m in e.children_named("surfaceMember") {
        for g in m.children.iter().filter(|c| is_geometry(c)) {
            match read_geometry(g, None).map(|r| r.geom) {
                Some(Geometry::Polygon(p)) => out.push(p),
                Some(Geometry::MultiPolygon(mp)) => out.extend(mp.0),
                _ => {}
            }
        }
    }
    if let Some(base) = e.child("baseSurface") {
        for g in base.children.iter().filter(|c| is_geometry(c)) {
            if let Some(Geometry::Polygon(p)) = read_geometry(g, None).map(|r| r.geom) {
                out.push(p);
            }
        }
    }
    out
}

/// Reads coordinates from `pos`, `posList`, `coordinates` or `coord` children.
fn read_coords(e: &Element, inherited_dim: Option<usize>) -> Vec<Coord<f64>> {
    if let Some(pl) = e.child("posList") {
        let dim = pl
            .attr_u32("srsDimension")
            .map(|d| d as usize)
            .or(inherited_dim)
            .unwrap_or(2)
            .max(2);
        return parse_numbers(pl.text_trim())
            .chunks(dim)
            .filter(|c| c.len() >= 2)
            .map(|c| Coord { x: c[0], y: c[1] })
            .collect();
    }
    let pos: Vec<Coord<f64>> = e
        .children_named("pos")
        .filter_map(|p| {
            let n = parse_numbers(p.text_trim());
            (n.len() >= 2).then(|| Coord { x: n[0], y: n[1] })
        })
        .collect();
    if !pos.is_empty() {
        return pos;
    }
    if let Some(c) = e.child("coordinates") {
        let cs = c.attr("cs").unwrap_or(",");
        let ts = c.attr("ts").unwrap_or(" ");
        let dec = c.attr("decimal").unwrap_or(".");
        let text = c.text_trim().replace(['\n', '\t', '\r'], " ");
        let tuples: Vec<&str> = if ts.trim().is_empty() {
            text.split_whitespace().collect()
        } else {
            text.split(ts).collect()
        };
        return tuples
            .into_iter()
            .filter_map(|t| {
                let parts: Vec<f64> = t
                    .split(cs)
                    .filter_map(|v| v.trim().replace(dec, ".").parse().ok())
                    .collect();
                (parts.len() >= 2).then(|| Coord {
                    x: parts[0],
                    y: parts[1],
                })
            })
            .collect();
    }
    e.children_named("coord")
        .filter_map(|c| {
            Some(Coord {
                x: c.child_text("X")?.parse().ok()?,
                y: c.child_text("Y")?.parse().ok()?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geom(xml: &str) -> Geometry<f64> {
        let e = Element::parse(xml.as_bytes()).unwrap();
        read_geometry_raw(&e).unwrap()
    }

    #[test]
    fn reads_gml2_and_gml3_shapes() {
        let g = geom(
            r#"<gml:Polygon xmlns:gml="http://www.opengis.net/gml"><gml:outerBoundaryIs><gml:LinearRing><gml:coordinates>0,0 10,0 10,10 0,10 0,0</gml:coordinates></gml:LinearRing></gml:outerBoundaryIs></gml:Polygon>"#,
        );
        match g {
            Geometry::Polygon(p) => assert_eq!(p.exterior().0.len(), 5),
            other => panic!("{other:?}"),
        }
        let g = geom(
            r#"<gml:MultiSurface xmlns:gml="http://www.opengis.net/gml/3.2"><gml:surfaceMember><gml:Polygon><gml:exterior><gml:LinearRing><gml:posList srsDimension="3">0 0 1 1 0 1 1 1 1 0 0 1</gml:posList></gml:LinearRing></gml:exterior></gml:Polygon></gml:surfaceMember><gml:surfaceMember><gml:Surface><gml:patches><gml:PolygonPatch><gml:exterior><gml:LinearRing><gml:posList>5 5 6 5 6 6 5 5</gml:posList></gml:LinearRing></gml:exterior></gml:PolygonPatch></gml:patches></gml:Surface></gml:surfaceMember></gml:MultiSurface>"#,
        );
        match g {
            Geometry::MultiPolygon(mp) => {
                assert_eq!(mp.0.len(), 2);
                assert_eq!(mp.0[0].exterior().0[1], Coord { x: 1.0, y: 0.0 });
            }
            other => panic!("{other:?}"),
        }
        let g = geom(
            r#"<gml:Curve xmlns:gml="http://www.opengis.net/gml/3.2"><gml:segments><gml:LineStringSegment><gml:posList>0 0 1 1</gml:posList></gml:LineStringSegment><gml:LineStringSegment><gml:posList>1 1 2 0</gml:posList></gml:LineStringSegment></gml:segments></gml:Curve>"#,
        );
        assert_eq!(
            g,
            Geometry::LineString(LineString::from(vec![(0.0, 0.0), (1.0, 1.0), (2.0, 0.0)]))
        );
    }

    #[test]
    fn scalars() {
        assert_eq!(scalar("12"), Value::from(12));
        assert_eq!(scalar("0123"), Value::from("0123"));
        assert_eq!(scalar("1.5"), Value::from(1.5));
        assert_eq!(scalar("true"), Value::Bool(true));
        assert_eq!(scalar("abc"), Value::from("abc"));
    }
}
