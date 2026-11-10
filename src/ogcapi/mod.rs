//! OGC API family (Common, Features, Records, Tiles).
//!
//! These are JSON/REST APIs; documents are deserialized with serde and keep
//! unknown members in `extra` maps so nothing returned by a server is lost.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use url::Url;

use crate::error::{Error, Result};
use crate::http::Client;

pub mod features;
pub mod records;
pub mod tiles;

pub use features::{Features, ItemsPage, ItemsQuery};
pub use records::{RecordView, Records};
pub use tiles::Tiles;

/// A hypermedia link.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Link {
    /// Target URL (may be a URI template when `templated` is true).
    pub href: String,
    /// Relation type.
    #[serde(default)]
    pub rel: Option<String>,
    /// Media type.
    #[serde(rename = "type", default)]
    pub type_: Option<String>,
    /// Title.
    #[serde(default)]
    pub title: Option<String>,
    /// Language.
    #[serde(default)]
    pub hreflang: Option<String>,
    /// Whether `href` is a template.
    #[serde(default)]
    pub templated: Option<bool>,
    /// Other members.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Helpers over link lists.
pub trait LinkExt {
    /// First link with relation `rel`, preferring JSON media types.
    fn find_rel(&self, rel: &str) -> Option<&Link>;
    /// All links with relation `rel`.
    fn all_rel(&self, rel: &str) -> Vec<&Link>;
}

fn rel_matches(link: &Link, rel: &str) -> bool {
    link.rel
        .as_deref()
        .map(|r| r == rel || r.rsplit('/').next() == Some(rel))
        .unwrap_or(false)
}

fn is_json(link: &Link) -> bool {
    link.type_
        .as_deref()
        .map(|t| t.contains("json"))
        .unwrap_or(true)
}

impl LinkExt for [Link] {
    fn find_rel(&self, rel: &str) -> Option<&Link> {
        self.iter()
            .find(|l| rel_matches(l, rel) && is_json(l))
            .or_else(|| self.iter().find(|l| rel_matches(l, rel)))
    }
    fn all_rel(&self, rel: &str) -> Vec<&Link> {
        self.iter().filter(|l| rel_matches(l, rel)).collect()
    }
}

impl LinkExt for Vec<Link> {
    fn find_rel(&self, rel: &str) -> Option<&Link> {
        self.as_slice().find_rel(rel)
    }
    fn all_rel(&self, rel: &str) -> Vec<&Link> {
        self.as_slice().all_rel(rel)
    }
}

/// Landing page (`/`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LandingPage {
    /// Title.
    #[serde(default)]
    pub title: Option<String>,
    /// Description.
    #[serde(default)]
    pub description: Option<String>,
    /// Links (`service-desc`, `conformance`, `data`, ...).
    #[serde(default)]
    pub links: Vec<Link>,
    /// Other members.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Conformance declaration (`/conformance`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conformance {
    /// Conformance class URIs.
    #[serde(rename = "conformsTo", default)]
    pub conforms_to: Vec<String>,
}

impl Conformance {
    /// Whether any class URI contains `fragment` (e.g. `ogcapi-features-1/1.0/conf/core`).
    pub fn has(&self, fragment: &str) -> bool {
        self.conforms_to.iter().any(|c| c.contains(fragment))
    }
}

/// Spatial extent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpatialExtent {
    /// Bounding boxes (`[minx, miny, maxx, maxy]` or 6 values with heights).
    #[serde(default)]
    pub bbox: Vec<Vec<f64>>,
    /// CRS of the boxes (CRS84 when absent).
    #[serde(default)]
    pub crs: Option<String>,
}

/// Temporal extent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemporalExtent {
    /// Intervals; `None` means open-ended.
    #[serde(default)]
    pub interval: Vec<Vec<Option<String>>>,
    /// Temporal reference system.
    #[serde(default)]
    pub trs: Option<String>,
}

/// Collection extent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Extent {
    /// Spatial part.
    #[serde(default)]
    pub spatial: Option<SpatialExtent>,
    /// Temporal part.
    #[serde(default)]
    pub temporal: Option<TemporalExtent>,
}

/// Keywords may be strings or `{ "keyword": ... }` objects.
pub(crate) fn lenient_keywords<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let v = Option::<Value>::deserialize(d)?;
    let mut out = Vec::new();
    let mut push = |x: &Value| match x {
        Value::String(s) => out.push(s.clone()),
        Value::Object(o) => {
            if let Some(Value::String(s)) = o.get("keyword").or_else(|| o.get("value")) {
                out.push(s.clone());
            }
        }
        _ => {}
    };
    match &v {
        Some(Value::Array(a)) => a.iter().for_each(&mut push),
        Some(x) => push(x),
        None => {}
    }
    Ok(out)
}

/// A collection (`/collections/{id}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Collection {
    /// Identifier.
    pub id: String,
    /// Title.
    #[serde(default)]
    pub title: Option<String>,
    /// Description.
    #[serde(default)]
    pub description: Option<String>,
    /// Keywords.
    #[serde(default, deserialize_with = "lenient_keywords")]
    pub keywords: Vec<String>,
    /// Links.
    #[serde(default)]
    pub links: Vec<Link>,
    /// Extent.
    #[serde(default)]
    pub extent: Option<Extent>,
    /// `feature`, `record`, ... (absent for coverage/map-only collections).
    #[serde(rename = "itemType", default)]
    pub item_type: Option<String>,
    /// Supported output CRSs.
    #[serde(default)]
    pub crs: Vec<String>,
    /// Storage CRS.
    #[serde(rename = "storageCrs", default)]
    pub storage_crs: Option<String>,
    /// Other members.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Collection {
    /// Whether the collection exposes `/items`.
    pub fn has_items(&self) -> bool {
        self.links.find_rel("items").is_some()
            || matches!(self.item_type.as_deref(), Some("feature") | Some("record"))
    }

    /// Whether the collection advertises tiles.
    pub fn has_tiles(&self) -> bool {
        self.links.iter().any(|l| {
            l.rel
                .as_deref()
                .map(|r| r.contains("tilesets"))
                .unwrap_or(false)
        })
    }
}

/// Collection list (`/collections`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Collections {
    /// Collections.
    #[serde(default)]
    pub collections: Vec<Collection>,
    /// Links.
    #[serde(default)]
    pub links: Vec<Link>,
    /// Other members.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Turns a JSON problem/exception body into an error.
pub(crate) fn api_error(bytes: &[u8]) -> Option<Error> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    api_error_value(&v)
}

pub(crate) fn api_error_value(v: &Value) -> Option<Error> {
    let o = v.as_object()?;
    // Real documents (landing pages, collections, features) carry links or features.
    if o.contains_key("links") || o.contains_key("features") || o.contains_key("geometry") {
        return None;
    }
    let as_text = |c: &Value| match c {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    };
    let (code, description) = if let Some(code) = o.get("code").and_then(as_text) {
        // pygeoapi / OGC API Common exception: {code, description}
        (
            code,
            o.get("description")
                .or_else(|| o.get("detail"))
                .and_then(Value::as_str)?,
        )
    } else if o.contains_key("status") || o.contains_key("detail") {
        // RFC 7807 problem details: {type, title, status, detail}
        let title = o.get("title").and_then(as_text);
        let status = o.get("status").and_then(as_text);
        (
            title.or(status)?,
            o.get("detail").and_then(Value::as_str).unwrap_or_default(),
        )
    } else {
        return None;
    };
    Some(Error::Api {
        code,
        description: description.to_string(),
    })
}

/// Client for the parts common to every OGC API (landing page,
/// conformance, collections).
#[derive(Debug, Clone)]
pub struct Api {
    root: Url,
    client: Client,
    format_param: bool,
}

impl Api {
    /// An API rooted at `url` (e.g. `https://demo.pygeoapi.io/master`).
    pub fn new(url: &str) -> Result<Api> {
        Self::with_client(Client::new(), url)
    }

    /// With a custom HTTP client.
    pub fn with_client(client: Client, url: &str) -> Result<Api> {
        let mut root = Url::parse(url.trim())?;
        // Drop `f=` so we control the format; keep other params (API keys).
        let keep: Vec<(String, String)> = root
            .query_pairs()
            .filter(|(k, _)| k != "f")
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        if keep.is_empty() {
            root.set_query(None);
        } else {
            root.query_pairs_mut().clear().extend_pairs(keep);
        }
        if !root.path().ends_with('/') {
            let p = format!("{}/", root.path());
            root.set_path(&p);
        }
        Ok(Api {
            root,
            client,
            format_param: false,
        })
    }

    /// Also send `f=json` (some servers ignore `Accept`).
    pub fn with_format_param(mut self, yes: bool) -> Api {
        self.format_param = yes;
        self
    }

    /// Root URL (with trailing slash).
    pub fn root(&self) -> &Url {
        &self.root
    }

    /// The HTTP client.
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Builds `root/seg1/seg2?query`.
    pub fn url(&self, segments: &[&str], query: &[(String, String)]) -> Result<Url> {
        let mut u = self.root.clone();
        {
            let mut segs = u
                .path_segments_mut()
                .map_err(|_| Error::InvalidRequest("URL cannot be a base".into()))?;
            segs.pop_if_empty();
            for s in segments {
                segs.push(s);
            }
        }
        if !query.is_empty() || self.format_param {
            let mut q = u.query_pairs_mut();
            for (k, v) in query {
                q.append_pair(k, v);
            }
            if self.format_param {
                q.append_pair("f", "json");
            }
        }
        Ok(u)
    }

    /// GETs a URL and deserializes the JSON body.
    pub async fn get_json_url<T: DeserializeOwned>(&self, url: &Url, accept: &str) -> Result<T> {
        let resp = self.client.get(url, Some(accept)).await?;
        let v: Value = serde_json::from_slice(&resp.body).map_err(|e| {
            if resp.body.starts_with(b"<") {
                Error::Parse(format!("expected JSON from {url}, got markup"))
            } else {
                Error::Json(e)
            }
        })?;
        if let Some(err) = api_error_value(&v) {
            return Err(err);
        }
        Ok(serde_json::from_value(v)?)
    }

    /// GETs `root/segments?query` as JSON.
    pub async fn get_json<T: DeserializeOwned>(
        &self,
        segments: &[&str],
        query: &[(String, String)],
    ) -> Result<T> {
        let url = self.url(segments, query)?;
        self.get_json_url(&url, "application/json").await
    }

    /// Landing page.
    pub async fn landing_page(&self) -> Result<LandingPage> {
        self.get_json(&[], &[]).await
    }

    /// Conformance classes.
    pub async fn conformance(&self) -> Result<Conformance> {
        self.get_json(&["conformance"], &[]).await
    }

    /// All collections.
    pub async fn collections(&self) -> Result<Collections> {
        self.get_json(&["collections"], &[]).await
    }

    /// One collection.
    pub async fn collection(&self, id: &str) -> Result<Collection> {
        self.get_json(&["collections", id], &[]).await
    }

    /// The OpenAPI document (`service-desc` link or `/api`), as JSON.
    pub async fn openapi(&self) -> Result<Value> {
        let url = self.url(&["api"], &[])?;
        self.get_json_url(
            &url,
            "application/vnd.oai.openapi+json;version=3.0, application/json",
        )
        .await
    }
}

macro_rules! deref_api {
    ($t:ty) => {
        impl std::ops::Deref for $t {
            type Target = Api;
            fn deref(&self) -> &Api {
                &self.api
            }
        }
    };
}
pub(crate) use deref_api;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_building() {
        let api = Api::new("https://example.com/ogc?f=html&key=1").unwrap();
        assert_eq!(api.root().as_str(), "https://example.com/ogc/?key=1");
        let u = api
            .url(&["collections", "a b"], &[("limit".into(), "5".into())])
            .unwrap();
        assert_eq!(
            u.as_str(),
            "https://example.com/ogc/collections/a%20b?key=1&limit=5"
        );
        let api = Api::new("https://example.com/")
            .unwrap()
            .with_format_param(true);
        assert_eq!(
            api.url(&["conformance"], &[]).unwrap().as_str(),
            "https://example.com/conformance?f=json"
        );
    }

    #[test]
    fn errors() {
        let e = api_error(br#"{"code":"NotFound","description":"no such collection"}"#);
        assert!(matches!(e, Some(Error::Api { .. })));
        assert!(api_error(br#"{"type":"FeatureCollection","features":[]}"#).is_none());
        assert!(api_error(br#"{"title":"Landing","description":"d","links":[]}"#).is_none());
        let p = api_error(
            br#"{"type":"about:blank","title":"Bad Request","status":400,"detail":"bad bbox"}"#,
        );
        assert!(matches!(p, Some(Error::Api { ref code, .. }) if code == "Bad Request"));
    }
}
