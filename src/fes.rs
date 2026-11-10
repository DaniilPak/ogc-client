//! OGC Filter Encoding (FES 1.1 for WFS 1.1 / CSW 2.0.2, FES 2.0 for WFS 2.0).
//!
//! ```
//! use ogc_client::fes::{Filter, FesVersion};
//!
//! let f = Filter::and([
//!     Filter::greater_than("POPULATION", 1_000_000),
//!     Filter::like("NAME", "San*"),
//!     Filter::bbox(None, (-130.0, 20.0, -60.0, 55.0), Some("EPSG:4326")),
//! ]);
//! let xml = f.to_xml(FesVersion::V2_0);
//! assert!(xml.starts_with("<fes:Filter"));
//! ```

use serde::{Deserialize, Serialize};

use crate::crs::{AxisOrder, Crs};
use crate::http::fmt_num;
use crate::xml::escape;

/// Filter encoding version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FesVersion {
    /// Filter Encoding 1.1 (`ogc:` namespace, `PropertyName`, GML 3.1.1 envelopes).
    V1_1,
    /// Filter Encoding 2.0 (`fes:` namespace, `ValueReference`, GML 3.2 envelopes).
    V2_0,
}

/// Binary comparison operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Comparison {
    /// `PropertyIsEqualTo`.
    EqualTo,
    /// `PropertyIsNotEqualTo`.
    NotEqualTo,
    /// `PropertyIsLessThan`.
    LessThan,
    /// `PropertyIsLessThanOrEqualTo`.
    LessThanOrEqualTo,
    /// `PropertyIsGreaterThan`.
    GreaterThan,
    /// `PropertyIsGreaterThanOrEqualTo`.
    GreaterThanOrEqualTo,
}

impl Comparison {
    fn element(&self) -> &'static str {
        match self {
            Comparison::EqualTo => "PropertyIsEqualTo",
            Comparison::NotEqualTo => "PropertyIsNotEqualTo",
            Comparison::LessThan => "PropertyIsLessThan",
            Comparison::LessThanOrEqualTo => "PropertyIsLessThanOrEqualTo",
            Comparison::GreaterThan => "PropertyIsGreaterThan",
            Comparison::GreaterThanOrEqualTo => "PropertyIsGreaterThanOrEqualTo",
        }
    }
}

/// A filter expression tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Filter {
    /// Binary comparison of a property with a literal.
    Compare {
        /// Operator.
        op: Comparison,
        /// Property name / value reference.
        property: String,
        /// Literal value.
        value: String,
        /// `matchCase` attribute.
        match_case: Option<bool>,
    },
    /// `PropertyIsLike` with `*`, `.` and `!` as wildcard, single char and escape.
    Like {
        /// Property name.
        property: String,
        /// Pattern.
        pattern: String,
        /// `matchCase` attribute.
        match_case: Option<bool>,
    },
    /// `PropertyIsNull`.
    IsNull(String),
    /// `PropertyIsBetween`.
    Between {
        /// Property name.
        property: String,
        /// Lower bound.
        lower: String,
        /// Upper bound.
        upper: String,
    },
    /// `BBOX` with an x/y bbox; axes are swapped for y/x CRSs.
    BBox {
        /// Geometry property (optional in FES 2.0).
        property: Option<String>,
        /// `(minx, miny, maxx, maxy)` in east/north order.
        bbox: (f64, f64, f64, f64),
        /// CRS of the bbox.
        crs: Option<String>,
    },
    /// Feature/record identifiers (`ResourceId` / `GmlObjectId`).
    Ids(Vec<String>),
    /// Logical AND.
    And(Vec<Filter>),
    /// Logical OR.
    Or(Vec<Filter>),
    /// Logical NOT.
    Not(Box<Filter>),
}

impl Filter {
    fn cmp(op: Comparison, property: &str, value: impl ToString) -> Filter {
        Filter::Compare {
            op,
            property: property.to_string(),
            value: value.to_string(),
            match_case: None,
        }
    }
    /// `property = value`.
    pub fn equal_to(property: &str, value: impl ToString) -> Filter {
        Self::cmp(Comparison::EqualTo, property, value)
    }
    /// `property <> value`.
    pub fn not_equal_to(property: &str, value: impl ToString) -> Filter {
        Self::cmp(Comparison::NotEqualTo, property, value)
    }
    /// `property < value`.
    pub fn less_than(property: &str, value: impl ToString) -> Filter {
        Self::cmp(Comparison::LessThan, property, value)
    }
    /// `property <= value`.
    pub fn less_than_or_equal_to(property: &str, value: impl ToString) -> Filter {
        Self::cmp(Comparison::LessThanOrEqualTo, property, value)
    }
    /// `property > value`.
    pub fn greater_than(property: &str, value: impl ToString) -> Filter {
        Self::cmp(Comparison::GreaterThan, property, value)
    }
    /// `property >= value`.
    pub fn greater_than_or_equal_to(property: &str, value: impl ToString) -> Filter {
        Self::cmp(Comparison::GreaterThanOrEqualTo, property, value)
    }
    /// `property LIKE pattern` (`*` any, `.` single, `!` escape).
    pub fn like(property: &str, pattern: &str) -> Filter {
        Filter::Like {
            property: property.to_string(),
            pattern: pattern.to_string(),
            match_case: None,
        }
    }
    /// `property IS NULL`.
    pub fn is_null(property: &str) -> Filter {
        Filter::IsNull(property.to_string())
    }
    /// `lower <= property <= upper`.
    pub fn between(property: &str, lower: impl ToString, upper: impl ToString) -> Filter {
        Filter::Between {
            property: property.to_string(),
            lower: lower.to_string(),
            upper: upper.to_string(),
        }
    }
    /// Bounding box filter (x/y order).
    pub fn bbox(property: Option<&str>, bbox: (f64, f64, f64, f64), crs: Option<&str>) -> Filter {
        Filter::BBox {
            property: property.map(str::to_string),
            bbox,
            crs: crs.map(str::to_string),
        }
    }
    /// Identifier filter.
    pub fn ids<I: IntoIterator<Item = S>, S: Into<String>>(ids: I) -> Filter {
        Filter::Ids(ids.into_iter().map(Into::into).collect())
    }
    /// AND of several filters.
    pub fn and<I: IntoIterator<Item = Filter>>(f: I) -> Filter {
        Filter::And(f.into_iter().collect())
    }
    /// OR of several filters.
    pub fn or<I: IntoIterator<Item = Filter>>(f: I) -> Filter {
        Filter::Or(f.into_iter().collect())
    }
    /// Negation.
    #[allow(clippy::should_implement_trait)]
    pub fn not(f: Filter) -> Filter {
        Filter::Not(Box::new(f))
    }
    /// Sets `matchCase` on comparison and like filters.
    pub fn match_case(mut self, yes: bool) -> Filter {
        match &mut self {
            Filter::Compare { match_case, .. } | Filter::Like { match_case, .. } => {
                *match_case = Some(yes)
            }
            _ => {}
        }
        self
    }

    /// Serializes as a complete `<Filter>` element with namespace declarations.
    pub fn to_xml(&self, version: FesVersion) -> String {
        let (p, ns, gml_ns) = match version {
            FesVersion::V1_1 => (
                "ogc",
                "http://www.opengis.net/ogc",
                "http://www.opengis.net/gml",
            ),
            FesVersion::V2_0 => (
                "fes",
                "http://www.opengis.net/fes/2.0",
                "http://www.opengis.net/gml/3.2",
            ),
        };
        let mut body = String::new();
        self.write(version, &mut body);
        format!("<{p}:Filter xmlns:{p}=\"{ns}\" xmlns:gml=\"{gml_ns}\">{body}</{p}:Filter>")
    }

    /// Serializes only the inner predicate (for embedding in a larger request
    /// that already declares the `ogc`/`fes` and `gml` prefixes).
    pub fn to_xml_fragment(&self, version: FesVersion) -> String {
        let mut body = String::new();
        self.write(version, &mut body);
        body
    }

    fn write(&self, v: FesVersion, out: &mut String) {
        let p = match v {
            FesVersion::V1_1 => "ogc",
            FesVersion::V2_0 => "fes",
        };
        let prop = |name: &str| match v {
            FesVersion::V1_1 => format!("<ogc:PropertyName>{}</ogc:PropertyName>", escape(name)),
            FesVersion::V2_0 => {
                format!("<fes:ValueReference>{}</fes:ValueReference>", escape(name))
            }
        };
        let lit = |val: &str| format!("<{p}:Literal>{}</{p}:Literal>", escape(val));
        let mc = |m: &Option<bool>| match m {
            Some(b) => format!(" matchCase=\"{b}\""),
            None => String::new(),
        };
        match self {
            Filter::Compare {
                op,
                property,
                value,
                match_case,
            } => {
                let el = op.element();
                out.push_str(&format!(
                    "<{p}:{el}{}>{}{}</{p}:{el}>",
                    mc(match_case),
                    prop(property),
                    lit(value)
                ));
            }
            Filter::Like {
                property,
                pattern,
                match_case,
            } => {
                let esc = match v {
                    FesVersion::V1_1 => "escape",
                    FesVersion::V2_0 => "escapeChar",
                };
                out.push_str(&format!(
                    "<{p}:PropertyIsLike wildCard=\"*\" singleChar=\".\" {esc}=\"!\"{}>{}{}</{p}:PropertyIsLike>",
                    mc(match_case),
                    prop(property),
                    lit(pattern)
                ));
            }
            Filter::IsNull(property) => {
                out.push_str(&format!(
                    "<{p}:PropertyIsNull>{}</{p}:PropertyIsNull>",
                    prop(property)
                ));
            }
            Filter::Between {
                property,
                lower,
                upper,
            } => {
                out.push_str(&format!(
                    "<{p}:PropertyIsBetween>{}<{p}:LowerBoundary>{}</{p}:LowerBoundary><{p}:UpperBoundary>{}</{p}:UpperBoundary></{p}:PropertyIsBetween>",
                    prop(property),
                    lit(lower),
                    lit(upper)
                ));
            }
            Filter::BBox {
                property,
                bbox,
                crs,
            } => {
                let parsed = crs.as_deref().and_then(Crs::parse);
                // GML 3 envelopes use the authority axis order of the srsName;
                // we always write URNs so the order is unambiguous.
                let (srs, swap) = match &parsed {
                    Some(c) => (
                        format!(" srsName=\"{}\"", escape(&c.to_urn())),
                        c.authority_axis_order() == AxisOrder::YX,
                    ),
                    None => (String::new(), false),
                };
                let (a, b, c, d) = *bbox;
                let (lo, hi) = if swap {
                    ((b, a), (d, c))
                } else {
                    ((a, b), (c, d))
                };
                let prop_xml = match (property, v) {
                    (Some(pn), _) => prop(pn),
                    (None, FesVersion::V1_1) => prop("the_geom"),
                    (None, FesVersion::V2_0) => String::new(),
                };
                out.push_str(&format!(
                    "<{p}:BBOX>{prop_xml}<gml:Envelope{srs}><gml:lowerCorner>{} {}</gml:lowerCorner><gml:upperCorner>{} {}</gml:upperCorner></gml:Envelope></{p}:BBOX>",
                    fmt_num(lo.0),
                    fmt_num(lo.1),
                    fmt_num(hi.0),
                    fmt_num(hi.1)
                ));
            }
            Filter::Ids(ids) => {
                for id in ids {
                    match v {
                        FesVersion::V1_1 => {
                            out.push_str(&format!("<ogc:GmlObjectId gml:id=\"{}\"/>", escape(id)))
                        }
                        FesVersion::V2_0 => {
                            out.push_str(&format!("<fes:ResourceId rid=\"{}\"/>", escape(id)))
                        }
                    }
                }
            }
            Filter::And(list) | Filter::Or(list) => {
                let el = if matches!(self, Filter::And(_)) {
                    "And"
                } else {
                    "Or"
                };
                if list.len() == 1 {
                    list[0].write(v, out);
                    return;
                }
                out.push_str(&format!("<{p}:{el}>"));
                for f in list {
                    f.write(v, out);
                }
                out.push_str(&format!("</{p}:{el}>"));
            }
            Filter::Not(f) => {
                out.push_str(&format!("<{p}:Not>"));
                f.write(v, out);
                out.push_str(&format!("</{p}:Not>"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xml::Element;

    #[test]
    fn fes2_round_trips_as_xml() {
        let f = Filter::and([
            Filter::equal_to("NAME", "A & B").match_case(false),
            Filter::bbox(None, (1.0, 2.0, 3.0, 4.0), Some("EPSG:4326")),
            Filter::not(Filter::is_null("POP")),
        ]);
        let xml = f.to_xml(FesVersion::V2_0);
        let root = Element::parse(xml.as_bytes()).unwrap();
        assert_eq!(root.name, "Filter");
        let and = root.child("And").unwrap();
        assert_eq!(and.children.len(), 3);
        let eq = and.child("PropertyIsEqualTo").unwrap();
        assert_eq!(eq.attr("matchCase"), Some("false"));
        assert_eq!(eq.child_text("Literal").as_deref(), Some("A & B"));
        let env = and.path(&["BBOX", "Envelope"]).unwrap();
        assert_eq!(env.attr("srsName"), Some("urn:ogc:def:crs:EPSG::4326"));
        // EPSG:4326 is lat/lon: axes swapped.
        assert_eq!(env.child_text("lowerCorner").as_deref(), Some("2 1"));
    }

    #[test]
    fn fes11_uses_ogc_namespace() {
        let xml = Filter::ids(["a.1", "a.2"]).to_xml(FesVersion::V1_1);
        assert!(xml.contains("<ogc:GmlObjectId gml:id=\"a.1\"/>"));
        let xml = Filter::like("title", "*soil*").to_xml(FesVersion::V1_1);
        assert!(xml.contains("escape=\"!\""));
        assert!(xml.contains("<ogc:PropertyName>title</ogc:PropertyName>"));
    }
}
