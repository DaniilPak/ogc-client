//! Catalogue Service for the Web (CSW) 2.0.2: GetCapabilities, GetRecords,
//! GetRecordById.
//!
//! ```no_run
//! # async fn run() -> ogc_client::Result<()> {
//! use ogc_client::csw::{CatalogueService, ElementSet, GetRecords};
//! use ogc_client::fes::Filter;
//!
//! let csw = CatalogueService::connect("https://demo.pycsw.org/cite/csw").await?;
//! let res = csw
//!     .get_records(
//!         &GetRecords::new()
//!             .filter(Filter::like("csw:AnyText", "*lorem*"))
//!             .element_set(ElementSet::Full)
//!             .max_records(5),
//!     )
//!     .await?;
//! for r in &res.records {
//!     println!("{:?} {:?}", r.identifier, r.title);
//! }
//! # Ok(()) }
//! ```

use serde::{Deserialize, Serialize};
use url::Url;

use crate::error::{Error, Result, parse_xml_checked};
use crate::fes::{FesVersion, Filter};
use crate::http::{Client, Kvp, build_url, clean_service_url};
use crate::ows::{
    BoundingBox, Operation, ServiceIdentification, ServiceProvider, operations_from_ows,
    ows_value_map,
};
use crate::xml::{Element, escape};

/// CSW 2.0.2 record namespace / output schema.
pub const CSW_NS: &str = "http://www.opengis.net/cat/csw/2.0.2";
/// ISO 19139 output schema.
pub const ISO_NS: &str = "http://www.isotc211.org/2005/gmd";

/// Parsed CSW capabilities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CswCapabilities {
    /// Version.
    pub version: String,
    /// Service identification.
    pub identification: ServiceIdentification,
    /// Service provider.
    pub provider: ServiceProvider,
    /// Operations.
    pub operations: Vec<Operation>,
    /// Service-wide constraints (`MaxRecordDefault`, `PostEncoding`, ...).
    pub constraints: std::collections::BTreeMap<String, Vec<String>>,
    /// Supported spatial operators.
    pub spatial_operators: Vec<String>,
    /// Supported comparison operators.
    pub comparison_operators: Vec<String>,
}

impl CswCapabilities {
    /// Parses a CSW 2.0.2 capabilities document.
    pub fn parse(bytes: &[u8]) -> Result<CswCapabilities> {
        let root = parse_xml_checked(bytes)?;
        if !root.is("Capabilities") {
            return Err(Error::Parse(format!(
                "expected csw:Capabilities, found <{}>",
                root.name
            )));
        }
        let om = root.child("OperationsMetadata");
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
        Ok(CswCapabilities {
            version: root.attr("version").unwrap_or("2.0.2").to_string(),
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
            spatial_operators: names(
                &["Spatial_Capabilities", "SpatialOperators"],
                "SpatialOperator",
            ),
            comparison_operators: names(
                &["Scalar_Capabilities", "ComparisonOperators"],
                "ComparisonOperator",
            ),
        })
    }

    /// Operation by name.
    pub fn operation(&self, name: &str) -> Option<&Operation> {
        self.operations
            .iter()
            .find(|o| o.name.eq_ignore_ascii_case(name))
    }

    /// Queryables advertised for GetRecords (`SupportedDublinCoreQueryables`, ...).
    pub fn queryables(&self) -> Vec<String> {
        self.operation("GetRecords")
            .map(|o| {
                o.constraints
                    .iter()
                    .filter(|(k, _)| k.contains("Queryables") && !k.starts_with("XPath"))
                    .flat_map(|(_, v)| v.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Output schemas supported by GetRecords.
    pub fn output_schemas(&self) -> Vec<String> {
        self.operation("GetRecords")
            .and_then(|o| {
                o.parameters
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("outputSchema"))
            })
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }
}

/// An `ows:BoundingBox`-like link reference (`dct:references`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reference {
    /// `scheme` attribute (e.g. `OGC:WMS`).
    pub scheme: Option<String>,
    /// URL.
    pub url: String,
}

/// A `dc:URI` link.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UriLink {
    /// `protocol` attribute.
    pub protocol: Option<String>,
    /// `name` attribute.
    pub name: Option<String>,
    /// `description` attribute.
    pub description: Option<String>,
    /// URL.
    pub url: String,
}

/// A catalogue record. Dublin Core records (`csw:Record`, `SummaryRecord`,
/// `BriefRecord`) are fully mapped; ISO 19139 records get the main fields and
/// keep their XML in [`CswRecord::xml`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CswRecord {
    /// Element name of the record (`Record`, `SummaryRecord`, `MD_Metadata`...).
    pub record_type: String,
    /// `dc:identifier` / `gmd:fileIdentifier`.
    pub identifier: Option<String>,
    /// `dc:type` / `gmd:hierarchyLevel`.
    pub type_: Option<String>,
    /// `dc:title`.
    pub title: Option<String>,
    /// `dct:alternative`.
    pub alternative: Option<String>,
    /// `dct:abstract` / `dc:description`.
    pub abstract_: Option<String>,
    /// `dc:subject` values.
    pub subjects: Vec<String>,
    /// `dc:format` values.
    pub formats: Vec<String>,
    /// `dc:creator`.
    pub creator: Option<String>,
    /// `dc:publisher`.
    pub publisher: Option<String>,
    /// `dc:contributor`.
    pub contributor: Option<String>,
    /// `dc:date`.
    pub date: Option<String>,
    /// `dct:modified`.
    pub modified: Option<String>,
    /// `dc:language`.
    pub language: Option<String>,
    /// `dc:rights` values.
    pub rights: Vec<String>,
    /// `dc:source`.
    pub source: Option<String>,
    /// `dc:relation`.
    pub relation: Option<String>,
    /// `dct:spatial`.
    pub spatial: Option<String>,
    /// `dct:references`.
    pub references: Vec<Reference>,
    /// `dc:URI` links.
    pub uris: Vec<UriLink>,
    /// `ows:BoundingBox` / `ows:WGS84BoundingBox` / ISO geographic extent.
    pub bbox: Option<BoundingBox>,
    /// The record's XML.
    pub xml: String,
}

impl CswRecord {
    /// Builds a record from its XML element.
    pub fn from_element(e: &Element) -> CswRecord {
        if e.is("MD_Metadata") || e.is("MI_Metadata") {
            return Self::from_iso(e);
        }
        let t = |n: &str| e.child_text(n);
        CswRecord {
            record_type: e.name.clone(),
            identifier: t("identifier"),
            type_: t("type"),
            title: t("title"),
            alternative: t("alternative"),
            abstract_: t("abstract").or_else(|| t("description")),
            subjects: e.children_text("subject"),
            formats: e.children_text("format"),
            creator: t("creator"),
            publisher: t("publisher"),
            contributor: t("contributor"),
            date: t("date"),
            modified: t("modified"),
            language: t("language"),
            rights: e.children_text("rights"),
            source: t("source"),
            relation: t("relation"),
            spatial: t("spatial"),
            references: e
                .children_named("references")
                .filter_map(|r| {
                    Some(Reference {
                        scheme: r.attr("scheme").map(str::to_string),
                        url: r.text_opt()?,
                    })
                })
                .collect(),
            uris: e
                .children_named("URI")
                .filter_map(|u| {
                    Some(UriLink {
                        protocol: u.attr("protocol").map(str::to_string),
                        name: u.attr("name").map(str::to_string),
                        description: u.attr("description").map(str::to_string),
                        url: u.text_opt()?,
                    })
                })
                .collect(),
            bbox: e
                .child_any(&["BoundingBox", "WGS84BoundingBox"])
                .and_then(BoundingBox::from_ows),
            xml: e.to_xml_string(),
        }
    }

    fn from_iso(e: &Element) -> CswRecord {
        let cs = |el: Option<&Element>| -> Option<String> {
            el.and_then(|x| {
                x.child_any(&["CharacterString", "Anchor"])
                    .and_then(Element::text_opt)
            })
        };
        let ident = e
            .child("identificationInfo")
            .and_then(|i| i.children.first());
        let citation = ident.and_then(|i| i.path(&["citation", "CI_Citation"]));
        let bbox = e
            .descendants()
            .find(|d| d.is("EX_GeographicBoundingBox"))
            .and_then(|b| {
                let n = |name: &str| {
                    b.child(name)
                        .and_then(|c| c.child("Decimal"))
                        .and_then(|d| d.text_trim().parse::<f64>().ok())
                };
                Some(BoundingBox::new(
                    n("westBoundLongitude")?,
                    n("southBoundLatitude")?,
                    n("eastBoundLongitude")?,
                    n("northBoundLatitude")?,
                    Some("CRS:84"),
                ))
            });
        let subjects = ident
            .map(|i| {
                i.descendants()
                    .filter(|d| d.is("keyword"))
                    .filter_map(|k| cs(Some(k)))
                    .collect()
            })
            .unwrap_or_default();
        let uris = e
            .descendants()
            .filter(|d| d.is("CI_OnlineResource"))
            .filter_map(|o| {
                Some(UriLink {
                    protocol: cs(o.child("protocol")),
                    name: cs(o.child("name")),
                    description: cs(o.child("description")),
                    url: o.path(&["linkage", "URL"])?.text_opt()?,
                })
            })
            .collect();
        CswRecord {
            record_type: e.name.clone(),
            identifier: cs(e.child("fileIdentifier")),
            type_: e
                .path(&["hierarchyLevel", "MD_ScopeCode"])
                .and_then(|c| c.attr("codeListValue").map(str::to_string)),
            title: cs(citation.and_then(|c| c.child("title"))),
            alternative: cs(citation.and_then(|c| c.child("alternateTitle"))),
            abstract_: cs(ident.and_then(|i| i.child("abstract"))),
            subjects,
            date: e
                .child("dateStamp")
                .and_then(|d| d.child_any(&["Date", "DateTime"]))
                .and_then(Element::text_opt),
            language: cs(e.child("language")).or_else(|| {
                e.path(&["language", "LanguageCode"])
                    .and_then(|l| l.attr("codeListValue").map(str::to_string))
            }),
            uris,
            bbox,
            xml: e.to_xml_string(),
            ..CswRecord::default()
        }
    }
}

/// Result of GetRecords / GetRecordById.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RecordsResponse {
    /// `numberOfRecordsMatched`.
    pub matched: Option<u64>,
    /// `numberOfRecordsReturned`.
    pub returned: Option<u64>,
    /// `nextRecord` (0 when there are no more).
    pub next_record: Option<u64>,
    /// `elementSet`.
    pub element_set: Option<String>,
    /// Records.
    pub records: Vec<CswRecord>,
}

impl RecordsResponse {
    /// Parses a `GetRecordsResponse` or `GetRecordByIdResponse`.
    pub fn parse(bytes: &[u8]) -> Result<RecordsResponse> {
        let root = parse_xml_checked(bytes)?;
        match root.name.as_str() {
            "GetRecordsResponse" => {
                let sr = root
                    .child("SearchResults")
                    .ok_or_else(|| Error::Parse("missing SearchResults".into()))?;
                let n = |a: &str| sr.attr(a).and_then(|v| v.trim().parse().ok());
                Ok(RecordsResponse {
                    matched: n("numberOfRecordsMatched"),
                    returned: n("numberOfRecordsReturned"),
                    next_record: n("nextRecord"),
                    element_set: sr.attr("elementSet").map(str::to_string),
                    records: sr.children.iter().map(CswRecord::from_element).collect(),
                })
            }
            "GetRecordByIdResponse" => {
                let records: Vec<CswRecord> =
                    root.children.iter().map(CswRecord::from_element).collect();
                Ok(RecordsResponse {
                    matched: Some(records.len() as u64),
                    returned: Some(records.len() as u64),
                    next_record: None,
                    element_set: None,
                    records,
                })
            }
            other => Err(Error::Parse(format!(
                "expected GetRecordsResponse, found <{other}>"
            ))),
        }
    }

    /// Record by identifier.
    pub fn record(&self, id: &str) -> Option<&CswRecord> {
        self.records
            .iter()
            .find(|r| r.identifier.as_deref() == Some(id))
    }
}

/// `ElementSetName`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ElementSet {
    /// `brief`.
    Brief,
    /// `summary` (default).
    #[default]
    Summary,
    /// `full`.
    Full,
}

impl ElementSet {
    fn as_str(&self) -> &'static str {
        match self {
            ElementSet::Brief => "brief",
            ElementSet::Summary => "summary",
            ElementSet::Full => "full",
        }
    }
}

/// A query constraint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Constraint {
    /// OGC Filter (FES 1.1).
    Filter(Filter),
    /// CQL text, e.g. `dc:title like '%water%'`.
    Cql(String),
}

/// A GetRecords request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GetRecords {
    /// `typeNames` (default `csw:Record`).
    pub type_names: String,
    /// Element set.
    pub element_set: ElementSet,
    /// Output schema (default CSW).
    pub output_schema: String,
    /// Constraint.
    pub constraint: Option<Constraint>,
    /// Sort property and ascending flag.
    pub sort_by: Option<(String, bool)>,
    /// 1-based start position.
    pub start_position: u64,
    /// Maximum records.
    pub max_records: u64,
    /// `resultType=hits`.
    pub hits_only: bool,
}

impl Default for GetRecords {
    fn default() -> Self {
        GetRecords::new()
    }
}

impl GetRecords {
    /// Defaults: `csw:Record`, summary, start 1, 10 records.
    pub fn new() -> GetRecords {
        GetRecords {
            type_names: "csw:Record".into(),
            element_set: ElementSet::Summary,
            output_schema: CSW_NS.into(),
            constraint: None,
            sort_by: None,
            start_position: 1,
            max_records: 10,
            hits_only: false,
        }
    }
    /// Uses an OGC filter.
    pub fn filter(mut self, f: Filter) -> GetRecords {
        self.constraint = Some(Constraint::Filter(f));
        self
    }
    /// Uses a CQL constraint.
    pub fn cql(mut self, cql: &str) -> GetRecords {
        self.constraint = Some(Constraint::Cql(cql.to_string()));
        self
    }
    /// Element set.
    pub fn element_set(mut self, e: ElementSet) -> GetRecords {
        self.element_set = e;
        self
    }
    /// Requests ISO 19139 (`gmd:MD_Metadata`) records.
    pub fn iso(mut self) -> GetRecords {
        self.output_schema = ISO_NS.into();
        self.type_names = "gmd:MD_Metadata".into();
        self
    }
    /// Output schema URI.
    pub fn output_schema(mut self, s: &str) -> GetRecords {
        self.output_schema = s.to_string();
        self
    }
    /// Sorting.
    pub fn sort_by(mut self, property: &str, ascending: bool) -> GetRecords {
        self.sort_by = Some((property.to_string(), ascending));
        self
    }
    /// 1-based start position.
    pub fn start_position(mut self, n: u64) -> GetRecords {
        self.start_position = n.max(1);
        self
    }
    /// Page size.
    pub fn max_records(mut self, n: u64) -> GetRecords {
        self.max_records = n;
        self
    }
    /// Only count matches.
    pub fn hits(mut self) -> GetRecords {
        self.hits_only = true;
        self
    }

    /// The POST XML body.
    pub fn to_xml(&self) -> String {
        let mut q = String::new();
        q.push_str(&format!(
            "<csw:ElementSetName>{}</csw:ElementSetName>",
            self.element_set.as_str()
        ));
        match &self.constraint {
            Some(Constraint::Filter(f)) => q.push_str(&format!(
                "<csw:Constraint version=\"1.1.0\">{}</csw:Constraint>",
                f.to_xml(FesVersion::V1_1)
            )),
            Some(Constraint::Cql(c)) => q.push_str(&format!(
                "<csw:Constraint version=\"1.1.0\"><csw:CqlText>{}</csw:CqlText></csw:Constraint>",
                escape(c)
            )),
            None => {}
        }
        if let Some((p, asc)) = &self.sort_by {
            q.push_str(&format!(
                "<ogc:SortBy><ogc:SortProperty><ogc:PropertyName>{}</ogc:PropertyName><ogc:SortOrder>{}</ogc:SortOrder></ogc:SortProperty></ogc:SortBy>",
                escape(p),
                if *asc { "ASC" } else { "DESC" }
            ));
        }
        format!(
            concat!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
                "<csw:GetRecords xmlns:csw=\"http://www.opengis.net/cat/csw/2.0.2\" ",
                "xmlns:ogc=\"http://www.opengis.net/ogc\" xmlns:gml=\"http://www.opengis.net/gml\" ",
                "xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dct=\"http://purl.org/dc/terms/\" ",
                "xmlns:ows=\"http://www.opengis.net/ows\" xmlns:gmd=\"http://www.isotc211.org/2005/gmd\" ",
                "service=\"CSW\" version=\"2.0.2\" resultType=\"{rt}\" startPosition=\"{sp}\" ",
                "maxRecords=\"{mr}\" outputSchema=\"{os}\" outputFormat=\"application/xml\">",
                "<csw:Query typeNames=\"{tn}\">{q}</csw:Query></csw:GetRecords>"
            ),
            rt = if self.hits_only { "hits" } else { "results" },
            sp = self.start_position,
            mr = self.max_records,
            os = escape(&self.output_schema),
            tn = escape(&self.type_names),
            q = q
        )
    }

    /// KVP parameters for HTTP GET.
    pub fn to_kvp(&self) -> Vec<(String, String)> {
        let mut k = Kvp::new();
        k.push("SERVICE", "CSW")
            .push("VERSION", "2.0.2")
            .push("REQUEST", "GetRecords")
            .push("TYPENAMES", self.type_names.clone())
            .push("ELEMENTSETNAME", self.element_set.as_str())
            .push(
                "RESULTTYPE",
                if self.hits_only { "hits" } else { "results" },
            )
            .push("OUTPUTSCHEMA", self.output_schema.clone())
            .push("OUTPUTFORMAT", "application/xml")
            .push("STARTPOSITION", self.start_position.to_string())
            .push("MAXRECORDS", self.max_records.to_string());
        if self.type_names.starts_with("gmd:") {
            k.push("NAMESPACE", format!("xmlns(gmd={ISO_NS})"));
        }
        match &self.constraint {
            Some(Constraint::Filter(f)) => {
                k.push("CONSTRAINTLANGUAGE", "FILTER")
                    .push("CONSTRAINT_LANGUAGE_VERSION", "1.1.0")
                    .push("CONSTRAINT", f.to_xml(FesVersion::V1_1));
            }
            Some(Constraint::Cql(c)) => {
                k.push("CONSTRAINTLANGUAGE", "CQL_TEXT")
                    .push("CONSTRAINT_LANGUAGE_VERSION", "1.1.0")
                    .push("CONSTRAINT", c.clone());
            }
            None => {}
        }
        if let Some((p, asc)) = &self.sort_by {
            k.push("SORTBY", format!("{p}:{}", if *asc { "A" } else { "D" }));
        }
        k.0
    }
}

/// How to send GetRecords.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HttpMethod {
    /// POST if advertised, else GET.
    #[default]
    Auto,
    /// Always GET (KVP).
    Get,
    /// Always POST (XML).
    Post,
}

/// A connected CSW.
#[derive(Debug, Clone)]
pub struct CatalogueService {
    url: Url,
    client: Client,
    capabilities: CswCapabilities,
    method: HttpMethod,
}

impl CatalogueService {
    /// Fetches capabilities with a default client.
    pub async fn connect(url: &str) -> Result<CatalogueService> {
        Self::connect_with(Client::new(), url).await
    }

    /// Fetches capabilities with the given client.
    pub async fn connect_with(client: Client, url: &str) -> Result<CatalogueService> {
        let caps_url = Self::capabilities_url(url)?;
        let resp = client
            .get(&caps_url, Some("application/xml, text/xml"))
            .await?;
        Ok(CatalogueService {
            url: clean_service_url(url)?,
            client,
            capabilities: CswCapabilities::parse(&resp.body)?,
            method: HttpMethod::Auto,
        })
    }

    /// Builds a service from a downloaded capabilities document.
    pub fn from_capabilities(client: Client, url: &str, xml: &[u8]) -> Result<CatalogueService> {
        Ok(CatalogueService {
            url: clean_service_url(url)?,
            client,
            capabilities: CswCapabilities::parse(xml)?,
            method: HttpMethod::Auto,
        })
    }

    /// The GetCapabilities URL.
    pub fn capabilities_url(url: &str) -> Result<Url> {
        let mut k = Kvp::new();
        k.push("SERVICE", "CSW")
            .push("REQUEST", "GetCapabilities")
            .push("VERSION", "2.0.2");
        build_url(clean_service_url(url)?.as_str(), &k.0)
    }

    /// Chooses GET or POST for GetRecords.
    pub fn with_method(mut self, m: HttpMethod) -> CatalogueService {
        self.method = m;
        self
    }

    /// Parsed capabilities.
    pub fn capabilities(&self) -> &CswCapabilities {
        &self.capabilities
    }

    /// The HTTP client.
    pub fn client(&self) -> &Client {
        &self.client
    }

    fn op_url(&self, op: &str, post: bool) -> Option<String> {
        let o = self.capabilities.operation(op)?;
        if post { o.post_url() } else { o.get_url() }.map(str::to_string)
    }

    /// The KVP GetRecords URL for a request.
    pub fn get_records_url(&self, req: &GetRecords) -> Result<Url> {
        let base = self
            .op_url("GetRecords", false)
            .unwrap_or_else(|| self.url.to_string());
        build_url(&base, &req.to_kvp())
    }

    /// Runs GetRecords.
    pub async fn get_records(&self, req: &GetRecords) -> Result<RecordsResponse> {
        let post_url = self.op_url("GetRecords", true);
        let use_post = match self.method {
            HttpMethod::Post => true,
            HttpMethod::Get => false,
            HttpMethod::Auto => post_url.is_some(),
        };
        let resp = if use_post {
            let url = Url::parse(&post_url.unwrap_or_else(|| self.url.to_string()))?;
            self.client
                .post(&url, req.to_xml(), "application/xml")
                .await?
        } else {
            let url = self.get_records_url(req)?;
            self.client.get(&url, None).await?
        };
        RecordsResponse::parse(&resp.body)
    }

    /// The GetRecordById URL.
    pub fn get_record_by_id_url(
        &self,
        ids: &[&str],
        element_set: ElementSet,
        output_schema: Option<&str>,
    ) -> Result<Url> {
        let base = self
            .op_url("GetRecordById", false)
            .unwrap_or_else(|| self.url.to_string());
        let mut k = Kvp::new();
        k.push("SERVICE", "CSW")
            .push("VERSION", "2.0.2")
            .push("REQUEST", "GetRecordById")
            .push("ID", ids.join(","))
            .push("ELEMENTSETNAME", element_set.as_str())
            .push("OUTPUTSCHEMA", output_schema.unwrap_or(CSW_NS))
            .push("OUTPUTFORMAT", "application/xml");
        build_url(&base, &k.0)
    }

    /// Fetches records by identifier.
    pub async fn get_record_by_id(
        &self,
        ids: &[&str],
        element_set: ElementSet,
        output_schema: Option<&str>,
    ) -> Result<RecordsResponse> {
        let url = self.get_record_by_id_url(ids, element_set, output_schema)?;
        let resp = self.client.get(&url, None).await?;
        RecordsResponse::parse(&resp.body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_records_xml_is_well_formed() {
        let req = GetRecords::new()
            .filter(Filter::and([
                Filter::like("csw:AnyText", "*water*"),
                Filter::bbox(
                    Some("ows:BoundingBox"),
                    (-10.0, 40.0, 10.0, 60.0),
                    Some("EPSG:4326"),
                ),
            ]))
            .sort_by("dc:title", true)
            .element_set(ElementSet::Full)
            .max_records(5);
        let root = Element::parse(req.to_xml().as_bytes()).unwrap();
        assert_eq!(root.name, "GetRecords");
        assert_eq!(root.attr("maxRecords"), Some("5"));
        let q = root.child("Query").unwrap();
        assert_eq!(q.child_text("ElementSetName").as_deref(), Some("full"));
        assert!(
            q.path(&["Constraint", "Filter", "And", "PropertyIsLike"])
                .is_some()
        );
        assert!(
            q.path(&["SortBy", "SortProperty", "PropertyName"])
                .is_some()
        );
        let cql = GetRecords::new().cql("dc:title like '%a&b%'");
        let root = Element::parse(cql.to_xml().as_bytes()).unwrap();
        assert_eq!(
            root.path_text(&["Query", "Constraint", "CqlText"])
                .as_deref(),
            Some("dc:title like '%a&b%'")
        );
    }
}
