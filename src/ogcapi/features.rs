//! OGC API – Features (Part 1 core, Part 2 CRS, Part 3 filtering queryables).
//!
//! ```no_run
//! # async fn run() -> ogc_client::Result<()> {
//! use ogc_client::ogcapi::{Features, ItemsQuery};
//!
//! let api = Features::new("https://demo.pygeoapi.io/master")?;
//! let mut page = api.items("lakes", &ItemsQuery::new().limit(10)).await?;
//! loop {
//!     for f in &page.features.features {
//!         println!("{:?}", f.property("name"));
//!     }
//!     match api.next_page(&page).await? {
//!         Some(next) => page = next,
//!         None => break,
//!     }
//! }
//! # Ok(()) }
//! ```

use serde_json::Value;
use url::Url;

use super::{Api, Link, LinkExt, deref_api};
use crate::error::Result;
use crate::http::{Client, fmt_num};

/// Query parameters for `/collections/{id}/items`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ItemsQuery {
    /// `limit`.
    pub limit: Option<u64>,
    /// `offset` (non-standard but widely supported).
    pub offset: Option<u64>,
    /// `bbox` (`minx,miny,maxx,maxy`, in `bbox-crs`, CRS84 by default).
    pub bbox: Option<[f64; 4]>,
    /// `bbox-crs`.
    pub bbox_crs: Option<String>,
    /// `datetime` (instant or interval `a/b`, `..` for open ends).
    pub datetime: Option<String>,
    /// Output `crs`.
    pub crs: Option<String>,
    /// Property equality filters (`name=value`).
    pub properties: Vec<(String, String)>,
    /// `properties` (property selection).
    pub select: Vec<String>,
    /// `filter` expression (CQL2 text by default).
    pub filter: Option<String>,
    /// `filter-lang` (e.g. `cql2-text`, `cql2-json`).
    pub filter_lang: Option<String>,
    /// `filter-crs`.
    pub filter_crs: Option<String>,
    /// `sortby` keys (`name`, `-name` for descending).
    pub sortby: Vec<String>,
    /// Free-text `q` terms (Records).
    pub q: Vec<String>,
    /// Other parameters.
    pub extra: Vec<(String, String)>,
}

impl ItemsQuery {
    /// Empty query.
    pub fn new() -> ItemsQuery {
        ItemsQuery::default()
    }
    /// Page size.
    pub fn limit(mut self, n: u64) -> ItemsQuery {
        self.limit = Some(n);
        self
    }
    /// Offset.
    pub fn offset(mut self, n: u64) -> ItemsQuery {
        self.offset = Some(n);
        self
    }
    /// Bounding box (lon/lat unless `bbox_crs` is set).
    pub fn bbox(mut self, minx: f64, miny: f64, maxx: f64, maxy: f64) -> ItemsQuery {
        self.bbox = Some([minx, miny, maxx, maxy]);
        self
    }
    /// `bbox-crs`.
    pub fn bbox_crs(mut self, crs: &str) -> ItemsQuery {
        self.bbox_crs = Some(crs.to_string());
        self
    }
    /// `datetime`.
    pub fn datetime(mut self, dt: &str) -> ItemsQuery {
        self.datetime = Some(dt.to_string());
        self
    }
    /// Output CRS.
    pub fn crs(mut self, crs: &str) -> ItemsQuery {
        self.crs = Some(crs.to_string());
        self
    }
    /// `name=value` property filter.
    pub fn property(mut self, name: &str, value: &str) -> ItemsQuery {
        self.properties.push((name.to_string(), value.to_string()));
        self
    }
    /// Selects returned properties.
    pub fn select<I: IntoIterator<Item = S>, S: Into<String>>(mut self, p: I) -> ItemsQuery {
        self.select = p.into_iter().map(Into::into).collect();
        self
    }
    /// CQL2 text filter.
    pub fn filter(mut self, cql2: &str) -> ItemsQuery {
        self.filter = Some(cql2.to_string());
        self.filter_lang.get_or_insert_with(|| "cql2-text".into());
        self
    }
    /// Filter language.
    pub fn filter_lang(mut self, lang: &str) -> ItemsQuery {
        self.filter_lang = Some(lang.to_string());
        self
    }
    /// Sort key (`-key` for descending).
    pub fn sortby(mut self, key: &str) -> ItemsQuery {
        self.sortby.push(key.to_string());
        self
    }
    /// Free-text search term (Records `q`).
    pub fn q(mut self, term: &str) -> ItemsQuery {
        self.q.push(term.to_string());
        self
    }
    /// Extra parameter.
    pub fn param(mut self, k: &str, v: &str) -> ItemsQuery {
        self.extra.push((k.to_string(), v.to_string()));
        self
    }

    /// Query parameters in request order.
    pub fn to_pairs(&self) -> Vec<(String, String)> {
        let mut p: Vec<(String, String)> = Vec::new();
        let mut push = |k: &str, v: String| p.push((k.to_string(), v));
        if let Some(n) = self.limit {
            push("limit", n.to_string());
        }
        if let Some(n) = self.offset {
            push("offset", n.to_string());
        }
        if let Some(b) = self.bbox {
            push(
                "bbox",
                b.iter().map(|x| fmt_num(*x)).collect::<Vec<_>>().join(","),
            );
        }
        if let Some(c) = &self.bbox_crs {
            push("bbox-crs", c.clone());
        }
        if let Some(d) = &self.datetime {
            push("datetime", d.clone());
        }
        if let Some(c) = &self.crs {
            push("crs", c.clone());
        }
        if !self.select.is_empty() {
            push("properties", self.select.join(","));
        }
        if let Some(f) = &self.filter {
            push("filter", f.clone());
        }
        if let Some(l) = &self.filter_lang
            && self.filter.is_some()
        {
            push("filter-lang", l.clone());
        }
        if let Some(c) = &self.filter_crs {
            push("filter-crs", c.clone());
        }
        if !self.sortby.is_empty() {
            push("sortby", self.sortby.join(","));
        }
        if !self.q.is_empty() {
            push("q", self.q.join(","));
        }
        for (k, v) in self.properties.iter().chain(self.extra.iter()) {
            push(k, v.clone());
        }
        p
    }
}

/// One page of items.
#[derive(Debug, Clone)]
pub struct ItemsPage {
    /// The GeoJSON feature collection as returned.
    pub features: geojson::FeatureCollection,
    /// `numberMatched`.
    pub number_matched: Option<u64>,
    /// `numberReturned`.
    pub number_returned: Option<u64>,
    /// Links (`next`, `prev`, `self`, ...).
    pub links: Vec<Link>,
    /// `timeStamp`.
    pub timestamp: Option<String>,
}

impl ItemsPage {
    /// Builds a page from a GeoJSON feature collection.
    pub fn from_collection(features: geojson::FeatureCollection) -> ItemsPage {
        let fm = features.foreign_members.as_ref();
        let get = |k: &str| fm.and_then(|m| m.get(k));
        let links = get("links")
            .and_then(|v| serde_json::from_value::<Vec<Link>>(v.clone()).ok())
            .unwrap_or_default();
        ItemsPage {
            number_matched: get("numberMatched").and_then(Value::as_u64),
            number_returned: get("numberReturned").and_then(Value::as_u64),
            timestamp: get("timeStamp").and_then(Value::as_str).map(str::to_string),
            links,
            features,
        }
    }

    /// Parses a GeoJSON items response.
    pub fn parse(bytes: &[u8]) -> Result<ItemsPage> {
        Ok(Self::from_collection(serde_json::from_slice(bytes)?))
    }

    /// URL of the next page, if any.
    pub fn next_url(&self) -> Option<&str> {
        self.links.find_rel("next").map(|l| l.href.as_str())
    }

    /// Number of features in this page.
    pub fn len(&self) -> usize {
        self.features.features.len()
    }

    /// Whether this page is empty.
    pub fn is_empty(&self) -> bool {
        self.features.features.is_empty()
    }
}

/// OGC API – Features client.
#[derive(Debug, Clone)]
pub struct Features {
    pub(crate) api: Api,
}

deref_api!(Features);

const GEOJSON_ACCEPT: &str = "application/geo+json, application/json;q=0.9";

impl Features {
    /// A client for the API rooted at `url`.
    pub fn new(url: &str) -> Result<Features> {
        Ok(Features {
            api: Api::new(url)?,
        })
    }

    /// With a custom HTTP client.
    pub fn with_client(client: Client, url: &str) -> Result<Features> {
        Ok(Features {
            api: Api::with_client(client, url)?,
        })
    }

    /// From an [`Api`].
    pub fn from_api(api: Api) -> Features {
        Features { api }
    }

    /// The items URL for a query (no network access).
    pub fn items_url(&self, collection: &str, query: &ItemsQuery) -> Result<Url> {
        self.api
            .url(&["collections", collection, "items"], &query.to_pairs())
    }

    /// Fetches one page of items.
    pub async fn items(&self, collection: &str, query: &ItemsQuery) -> Result<ItemsPage> {
        let url = self.items_url(collection, query)?;
        let fc: geojson::FeatureCollection = self.api.get_json_url(&url, GEOJSON_ACCEPT).await?;
        Ok(ItemsPage::from_collection(fc))
    }

    /// Follows the `next` link of a page.
    pub async fn next_page(&self, page: &ItemsPage) -> Result<Option<ItemsPage>> {
        let Some(next) = page.next_url() else {
            return Ok(None);
        };
        let url = Url::parse(next)?;
        let fc: geojson::FeatureCollection = self.api.get_json_url(&url, GEOJSON_ACCEPT).await?;
        let page = ItemsPage::from_collection(fc);
        Ok(if page.is_empty() { None } else { Some(page) })
    }

    /// Fetches all items, following `next` links, up to `max` features.
    pub async fn all_items(
        &self,
        collection: &str,
        query: &ItemsQuery,
        max: Option<usize>,
    ) -> Result<Vec<geojson::Feature>> {
        let mut out = Vec::new();
        let mut page = self.items(collection, query).await?;
        loop {
            out.append(&mut page.features.features);
            if let Some(m) = max
                && out.len() >= m
            {
                out.truncate(m);
                break;
            }
            match self.next_page(&page).await? {
                Some(p) => page = p,
                None => break,
            }
        }
        Ok(out)
    }

    /// One feature by id.
    pub async fn item(&self, collection: &str, id: &str) -> Result<geojson::Feature> {
        let url = self
            .api
            .url(&["collections", collection, "items", id], &[])?;
        self.api.get_json_url(&url, GEOJSON_ACCEPT).await
    }

    /// Queryables (JSON Schema) of a collection.
    pub async fn queryables(&self, collection: &str) -> Result<Value> {
        let url = self
            .api
            .url(&["collections", collection, "queryables"], &[])?;
        self.api
            .get_json_url(&url, "application/schema+json, application/json")
            .await
    }
}

/// Names of the properties in a queryables JSON Schema.
pub fn queryable_names(schema: &Value) -> Vec<String> {
    schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_pairs() {
        let q = ItemsQuery::new()
            .limit(5)
            .bbox(-10.0, 40.5, 10.0, 60.0)
            .datetime("2020-01-01/..")
            .filter("name LIKE 'A%'")
            .property("type", "lake")
            .sortby("-name");
        let p = q.to_pairs();
        assert!(p.contains(&("bbox".into(), "-10,40.5,10,60".into())));
        assert!(p.contains(&("filter-lang".into(), "cql2-text".into())));
        assert!(p.contains(&("type".into(), "lake".into())));
        assert!(p.contains(&("sortby".into(), "-name".into())));
    }
}
