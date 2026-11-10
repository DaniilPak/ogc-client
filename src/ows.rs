//! Service metadata shared by all OGC XML services: identification,
//! provider/contact, operations and bounding boxes.
//!
//! These types parse both the OWS Common 1.0/1.1/2.0 encodings (WMTS, WFS
//! 1.1/2.0, CSW) and the older WMS `<Service>`/`<Capability>` encodings.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::crs::{AxisOrder, Crs};
use crate::xml::{Element, parse_numbers};

/// `(minx, miny, maxx, maxy)`.
pub type Bbox = (f64, f64, f64, f64);

/// A bounding box as advertised by a service.
///
/// `minx/miny/maxx/maxy` are stored exactly as written in the document. For
/// WMS 1.3.0 and OWS bounding boxes in a y/x CRS (e.g. EPSG:4326) "x" therefore
/// holds latitude; use [`BoundingBox::to_xy`] to get easting/longitude first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoundingBox {
    /// CRS identifier, if given (`None` means CRS:84 for geographic boxes).
    pub crs: Option<String>,
    /// First coordinate of the lower corner.
    pub minx: f64,
    /// Second coordinate of the lower corner.
    pub miny: f64,
    /// First coordinate of the upper corner.
    pub maxx: f64,
    /// Second coordinate of the upper corner.
    pub maxy: f64,
    /// Spatial resolution along the first axis (WMS `resx`).
    pub resx: Option<f64>,
    /// Spatial resolution along the second axis (WMS `resy`).
    pub resy: Option<f64>,
}

impl BoundingBox {
    /// A box from x/y values with an optional CRS.
    pub fn new(minx: f64, miny: f64, maxx: f64, maxy: f64, crs: Option<&str>) -> Self {
        BoundingBox {
            crs: crs.map(str::to_string),
            minx,
            miny,
            maxx,
            maxy,
            resx: None,
            resy: None,
        }
    }

    /// Parses `ows:BoundingBox` / `ows:WGS84BoundingBox` (LowerCorner/UpperCorner).
    pub(crate) fn from_ows(e: &Element) -> Option<BoundingBox> {
        let lower = parse_numbers(e.child_text("LowerCorner")?.as_str());
        let upper = parse_numbers(e.child_text("UpperCorner")?.as_str());
        if lower.len() < 2 || upper.len() < 2 {
            return None;
        }
        let crs = e.attr("crs").map(str::to_string).or_else(|| {
            if e.is("WGS84BoundingBox") {
                Some("urn:ogc:def:crs:OGC:2:84".to_string())
            } else {
                None
            }
        });
        Some(BoundingBox {
            crs,
            minx: lower[0],
            miny: lower[1],
            maxx: upper[0],
            maxy: upper[1],
            resx: None,
            resy: None,
        })
    }

    /// Parses WMS `BoundingBox`/`LatLonBoundingBox` attribute boxes.
    pub(crate) fn from_attrs(e: &Element) -> Option<BoundingBox> {
        Some(BoundingBox {
            crs: e.attr("CRS").or_else(|| e.attr("SRS")).map(str::to_string),
            minx: e.attr_f64("minx")?,
            miny: e.attr_f64("miny")?,
            maxx: e.attr_f64("maxx")?,
            maxy: e.attr_f64("maxy")?,
            resx: e.attr_f64("resx"),
            resy: e.attr_f64("resy"),
        })
    }

    /// Parses WMS 1.3.0 `EX_GeographicBoundingBox`.
    pub(crate) fn from_ex_geographic(e: &Element) -> Option<BoundingBox> {
        let num = |n: &str| e.child_text(n).and_then(|t| t.parse::<f64>().ok());
        Some(BoundingBox {
            crs: Some("CRS:84".to_string()),
            minx: num("westBoundLongitude")?,
            miny: num("southBoundLatitude")?,
            maxx: num("eastBoundLongitude")?,
            maxy: num("northBoundLatitude")?,
            resx: None,
            resy: None,
        })
    }

    /// Parsed CRS, if any.
    pub fn crs(&self) -> Option<Crs> {
        self.crs.as_deref().and_then(Crs::parse)
    }

    /// Returns `(minx, miny, maxx, maxy)` with easting/longitude first, given
    /// whether the document stored coordinates in the CRS' authority order
    /// (`true` for WMS 1.3.0, OWS/WMTS/WFS 1.1+, `false` for WMS 1.1.1).
    pub fn to_xy(&self, authority_order: bool) -> (f64, f64, f64, f64) {
        let swap = authority_order
            && self
                .crs()
                .map(|c| c.authority_axis_order() == AxisOrder::YX)
                .unwrap_or(false);
        if swap {
            (self.miny, self.minx, self.maxy, self.maxx)
        } else {
            (self.minx, self.miny, self.maxx, self.maxy)
        }
    }

    /// As a `geo_types::Rect` in the stored order.
    pub fn to_rect(&self) -> geo_types::Rect<f64> {
        geo_types::Rect::new(
            geo_types::coord! { x: self.minx, y: self.miny },
            geo_types::coord! { x: self.maxx, y: self.maxy },
        )
    }
}

/// Service identification (`ows:ServiceIdentification` or WMS `<Service>`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ServiceIdentification {
    /// Service type, e.g. `WMS`, `WFS`, `OGC:WMS`.
    pub service_type: Option<String>,
    /// Supported versions advertised by the service.
    pub versions: Vec<String>,
    /// Human-readable title.
    pub title: Option<String>,
    /// Abstract.
    pub abstract_: Option<String>,
    /// Keywords.
    pub keywords: Vec<String>,
    /// Fees.
    pub fees: Option<String>,
    /// Access constraints.
    pub access_constraints: Option<String>,
    /// Profiles (OWS).
    pub profiles: Vec<String>,
}

impl ServiceIdentification {
    pub(crate) fn from_ows(e: &Element) -> Self {
        ServiceIdentification {
            service_type: e.child_text("ServiceType"),
            versions: e.children_text("ServiceTypeVersion"),
            title: e.child_text("Title"),
            abstract_: e.child_text("Abstract"),
            keywords: keywords(e),
            fees: e.child_text("Fees"),
            access_constraints: e.child_text("AccessConstraints"),
            profiles: e.children_text("Profile"),
        }
    }
}

/// Collects `Keywords/Keyword` and WMS `KeywordList/Keyword` texts.
pub(crate) fn keywords(e: &Element) -> Vec<String> {
    let mut out = Vec::new();
    for kw in e
        .children
        .iter()
        .filter(|c| c.is("Keywords") || c.is("KeywordList"))
    {
        out.extend(kw.children_text("Keyword"));
        // WMS 1.1.1 sometimes puts space separated keywords in the element text.
        if kw.children.is_empty()
            && let Some(t) = kw.text_opt()
        {
            out.extend(t.split_whitespace().map(str::to_string));
        }
    }
    out
}

/// Contact information of the service provider.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Contact {
    /// Person name.
    pub name: Option<String>,
    /// Organization (WMS `ContactOrganization`).
    pub organization: Option<String>,
    /// Position name.
    pub position: Option<String>,
    /// Role (OWS).
    pub role: Option<String>,
    /// Telephone.
    pub phone: Option<String>,
    /// Fax.
    pub fax: Option<String>,
    /// Street address.
    pub address: Option<String>,
    /// City.
    pub city: Option<String>,
    /// State or province.
    pub region: Option<String>,
    /// Postal code.
    pub postcode: Option<String>,
    /// Country.
    pub country: Option<String>,
    /// E-mail address.
    pub email: Option<String>,
    /// Hours of service.
    pub hours: Option<String>,
    /// Contact instructions.
    pub instructions: Option<String>,
    /// Online resource.
    pub url: Option<String>,
}

/// Service provider (`ows:ServiceProvider` or WMS `ContactInformation`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ServiceProvider {
    /// Provider name.
    pub name: Option<String>,
    /// Provider site URL.
    pub url: Option<String>,
    /// Contact details.
    pub contact: Contact,
}

impl ServiceProvider {
    pub(crate) fn from_ows(e: &Element) -> Self {
        let sc = e.child("ServiceContact");
        let ci = sc.and_then(|s| s.child("ContactInfo"));
        let addr = ci.and_then(|c| c.child("Address"));
        let phone = ci.and_then(|c| c.child("Phone"));
        let t = |el: Option<&Element>, n: &str| el.and_then(|x| x.child_text(n));
        ServiceProvider {
            name: e.child_text("ProviderName"),
            url: e
                .child("ProviderSite")
                .and_then(|s| s.attr("href"))
                .map(str::to_string),
            contact: Contact {
                name: t(sc, "IndividualName"),
                organization: e.child_text("ProviderName"),
                position: t(sc, "PositionName"),
                role: t(sc, "Role"),
                phone: t(phone, "Voice"),
                fax: t(phone, "Facsimile"),
                address: t(addr, "DeliveryPoint"),
                city: t(addr, "City"),
                region: t(addr, "AdministrativeArea"),
                postcode: t(addr, "PostalCode"),
                country: t(addr, "Country"),
                email: t(addr, "ElectronicMailAddress"),
                hours: t(ci, "HoursOfService"),
                instructions: t(ci, "ContactInstructions"),
                url: ci
                    .and_then(|c| c.child("OnlineResource"))
                    .and_then(|o| o.attr("href"))
                    .map(str::to_string),
            },
        }
    }

    /// Parses WMS `ContactInformation`.
    pub(crate) fn from_wms(service: &Element) -> Self {
        let ci = service.child("ContactInformation");
        let person = ci.and_then(|c| c.child("ContactPersonPrimary"));
        let addr = ci.and_then(|c| c.child("ContactAddress"));
        let t = |el: Option<&Element>, n: &str| el.and_then(|x| x.child_text(n));
        let org = t(person, "ContactOrganization");
        ServiceProvider {
            name: org.clone(),
            url: service
                .child("OnlineResource")
                .and_then(|o| o.attr("href"))
                .map(str::to_string),
            contact: Contact {
                name: t(person, "ContactPerson"),
                organization: org,
                position: t(ci, "ContactPosition"),
                role: None,
                phone: t(ci, "ContactVoiceTelephone"),
                fax: t(ci, "ContactFacsimileTelephone"),
                address: t(addr, "Address"),
                city: t(addr, "City"),
                region: t(addr, "StateOrProvince"),
                postcode: t(addr, "PostCode"),
                country: t(addr, "Country"),
                email: t(ci, "ContactElectronicMailAddress"),
                hours: None,
                instructions: None,
                url: None,
            },
        }
    }
}

/// An HTTP binding (DCP) of an operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperationMethod {
    /// `Get` or `Post`.
    pub method: String,
    /// Endpoint URL.
    pub url: String,
    /// Constraints on this binding (e.g. WMTS `GetEncoding` = `KVP`/`RESTful`).
    pub constraints: BTreeMap<String, Vec<String>>,
}

/// An operation offered by the service (`GetMap`, `GetFeature`, ...).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Operation {
    /// Operation name.
    pub name: String,
    /// HTTP bindings.
    pub methods: Vec<OperationMethod>,
    /// Output formats (WMS `Format`, or the OWS `outputFormat`/`Format` parameter).
    pub formats: Vec<String>,
    /// OWS parameters and their allowed values.
    pub parameters: BTreeMap<String, Vec<String>>,
    /// OWS operation constraints.
    pub constraints: BTreeMap<String, Vec<String>>,
}

impl Operation {
    /// URL for HTTP GET, if advertised.
    pub fn get_url(&self) -> Option<&str> {
        self.methods
            .iter()
            .find(|m| m.method.eq_ignore_ascii_case("get"))
            .map(|m| m.url.as_str())
    }

    /// URL for HTTP POST, if advertised.
    pub fn post_url(&self) -> Option<&str> {
        self.methods
            .iter()
            .find(|m| m.method.eq_ignore_ascii_case("post"))
            .map(|m| m.url.as_str())
    }

    /// GET URL whose `GetEncoding` constraint allows `encoding` (e.g. `KVP`),
    /// or which has no encoding constraint.
    pub fn get_url_for_encoding(&self, encoding: &str) -> Option<&str> {
        let gets = || {
            self.methods
                .iter()
                .filter(|m| m.method.eq_ignore_ascii_case("get"))
        };
        gets()
            .find(|m| {
                m.constraints
                    .get("GetEncoding")
                    .map(|v| v.iter().any(|x| x.eq_ignore_ascii_case(encoding)))
                    .unwrap_or(false)
            })
            .or_else(|| gets().find(|m| !m.constraints.contains_key("GetEncoding")))
            .map(|m| m.url.as_str())
    }

    /// Parses `ows:Operation`.
    pub(crate) fn from_ows(e: &Element) -> Operation {
        let mut methods = Vec::new();
        for dcp in e.children_named("DCP") {
            for http in dcp.children_named("HTTP") {
                for m in &http.children {
                    if let Some(url) = m.attr("href") {
                        methods.push(OperationMethod {
                            method: m.name.clone(),
                            url: url.to_string(),
                            constraints: ows_value_map(m, "Constraint"),
                        });
                    }
                }
            }
        }
        let parameters = ows_value_map(e, "Parameter");
        let formats = parameters
            .iter()
            .find(|(k, _)| {
                k.eq_ignore_ascii_case("outputFormat") || k.eq_ignore_ascii_case("Format")
            })
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        Operation {
            name: e.attr("name").unwrap_or_default().to_string(),
            methods,
            formats,
            parameters,
            constraints: ows_value_map(e, "Constraint"),
        }
    }

    /// Parses a WMS 1.x `<Request>` child (e.g. `<GetMap>`).
    pub(crate) fn from_wms(e: &Element) -> Operation {
        let mut methods = Vec::new();
        for dcp in e.children_named("DCPType") {
            for http in dcp.children_named("HTTP") {
                for m in &http.children {
                    if let Some(url) = m.child("OnlineResource").and_then(|o| o.attr("href")) {
                        methods.push(OperationMethod {
                            method: m.name.clone(),
                            url: url.to_string(),
                            constraints: BTreeMap::new(),
                        });
                    }
                }
            }
        }
        Operation {
            name: e.name.clone(),
            methods,
            formats: e.children_text("Format"),
            parameters: BTreeMap::new(),
            constraints: BTreeMap::new(),
        }
    }
}

/// Parses `ows:Parameter`/`ows:Constraint` children into name → allowed values.
pub(crate) fn ows_value_map(e: &Element, child: &str) -> BTreeMap<String, Vec<String>> {
    let mut map = BTreeMap::new();
    for p in e.children_named(child) {
        let name = p.attr("name").unwrap_or_default().to_string();
        let mut values: Vec<String> = Vec::new();
        for d in p.descendants().skip(1) {
            if d.is("Value")
                && let Some(t) = d.text_opt()
            {
                values.push(t);
            }
        }
        if let Some(dv) = p.child_text("DefaultValue")
            && values.is_empty()
        {
            values.push(dv);
        }
        map.insert(name, values);
    }
    map
}

/// Parses `ows:OperationsMetadata`.
pub(crate) fn operations_from_ows(e: &Element) -> Vec<Operation> {
    e.children_named("Operation")
        .map(Operation::from_ows)
        .collect()
}

/// A metadata or data link (`MetadataURL`, `DataURL`, `ows:Metadata`...).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetadataLink {
    /// Link type (`type` attribute, e.g. `ISO19115:2003`, or MIME type).
    pub type_: Option<String>,
    /// MIME format, if given.
    pub format: Option<String>,
    /// URL.
    pub url: String,
}

impl MetadataLink {
    pub(crate) fn from_wms(e: &Element) -> Option<MetadataLink> {
        let url = e
            .child("OnlineResource")
            .and_then(|o| o.attr("href"))
            .or_else(|| e.attr("href"))
            .map(str::to_string)
            .or_else(|| e.text_opt())?;
        Some(MetadataLink {
            type_: e.attr("type").map(str::to_string),
            format: e
                .child_text("Format")
                .or_else(|| e.attr("format").map(str::to_string)),
            url,
        })
    }
}
