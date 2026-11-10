//! OGC API – Records (Part 1).
//!
//! Records are GeoJSON features whose `properties` carry catalogue fields
//! (`title`, `description`, `keywords`, `type`, ...). [`RecordView`] gives
//! typed access to those fields.
//!
//! ```no_run
//! # async fn run() -> ogc_client::Result<()> {
//! use ogc_client::ogcapi::{ItemsQuery, Records, RecordView};
//!
//! let api = Records::new("https://demo.pygeoapi.io/master")?;
//! let page = api.records("dutch-metadata", &ItemsQuery::new().q("water").limit(5)).await?;
//! for f in &page.features.features {
//!     println!("{:?}", RecordView(f).title());
//! }
//! # Ok(()) }
//! ```

use serde_json::Value;

use super::{Api, Collection, ItemsPage, ItemsQuery, Link, deref_api};
use crate::error::Result;
use crate::http::Client;
use crate::ogcapi::Features;

/// OGC API – Records client.
#[derive(Debug, Clone)]
pub struct Records {
    pub(crate) api: Api,
}

deref_api!(Records);

impl Records {
    /// A client for the API rooted at `url`.
    pub fn new(url: &str) -> Result<Records> {
        Ok(Records {
            api: Api::new(url)?,
        })
    }

    /// With a custom HTTP client.
    pub fn with_client(client: Client, url: &str) -> Result<Records> {
        Ok(Records {
            api: Api::with_client(client, url)?,
        })
    }

    fn features(&self) -> Features {
        Features::from_api(self.api.clone())
    }

    /// Collections with `itemType: record` (catalogues).
    pub async fn catalogues(&self) -> Result<Vec<Collection>> {
        Ok(self
            .api
            .collections()
            .await?
            .collections
            .into_iter()
            .filter(|c| c.item_type.as_deref() == Some("record"))
            .collect())
    }

    /// Searches records in a catalogue (`q`, `bbox`, `datetime`, `filter`, ...).
    pub async fn records(&self, catalogue: &str, query: &ItemsQuery) -> Result<ItemsPage> {
        self.features().items(catalogue, query).await
    }

    /// Follows the `next` link.
    pub async fn next_page(&self, page: &ItemsPage) -> Result<Option<ItemsPage>> {
        self.features().next_page(page).await
    }

    /// One record by id.
    pub async fn record(&self, catalogue: &str, id: &str) -> Result<geojson::Feature> {
        self.features().item(catalogue, id).await
    }

    /// Queryables of a catalogue.
    pub async fn queryables(&self, catalogue: &str) -> Result<Value> {
        self.features().queryables(catalogue).await
    }
}

/// Typed read access to a record's properties.
#[derive(Debug, Clone, Copy)]
pub struct RecordView<'a>(pub &'a geojson::Feature);

impl<'a> RecordView<'a> {
    fn prop(&self, k: &str) -> Option<&'a Value> {
        self.0.properties.as_ref()?.get(k)
    }
    fn s(&self, k: &str) -> Option<&'a str> {
        self.prop(k).and_then(Value::as_str)
    }

    /// Record id.
    pub fn id(&self) -> Option<String> {
        self.0.id.as_ref().map(|id| match id {
            geojson::feature::Id::String(s) => s.clone(),
            geojson::feature::Id::Number(n) => n.to_string(),
        })
    }
    /// `title`.
    pub fn title(&self) -> Option<&'a str> {
        self.s("title")
    }
    /// `description`.
    pub fn description(&self) -> Option<&'a str> {
        self.s("description")
    }
    /// `type` (e.g. `dataset`, `service`).
    pub fn type_(&self) -> Option<&'a str> {
        self.s("type")
    }
    /// `created`.
    pub fn created(&self) -> Option<&'a str> {
        self.s("created")
    }
    /// `updated`.
    pub fn updated(&self) -> Option<&'a str> {
        self.s("updated")
    }
    /// `language` (string or `{code}` object).
    pub fn language(&self) -> Option<&'a str> {
        match self.prop("language")? {
            Value::String(s) => Some(s),
            Value::Object(o) => o.get("code").and_then(Value::as_str),
            _ => None,
        }
    }
    /// `keywords` plus the terms of `themes[].concepts[]`.
    pub fn keywords(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .prop("keywords")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        if let Some(themes) = self.prop("themes").and_then(Value::as_array) {
            for t in themes {
                if let Some(concepts) = t.get("concepts").and_then(Value::as_array) {
                    for c in concepts {
                        if let Some(id) = c.get("id").and_then(Value::as_str) {
                            out.push(id.to_string());
                        }
                    }
                }
            }
        }
        out
    }
    /// `contacts` as raw JSON.
    pub fn contacts(&self) -> Vec<&'a Value> {
        self.prop("contacts")
            .and_then(Value::as_array)
            .map(|a| a.iter().collect())
            .unwrap_or_default()
    }
    /// Record links (top-level `links`, which point to the described resources).
    pub fn links(&self) -> Vec<Link> {
        self.0
            .foreign_members
            .as_ref()
            .and_then(|m| m.get("links"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default()
    }
    /// `time` member as raw JSON.
    pub fn time(&self) -> Option<&'a Value> {
        self.0.foreign_members.as_ref()?.get("time")
    }
}
