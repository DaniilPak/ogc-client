//! Synchronous wrappers (feature `blocking`).
//!
//! Each wrapper owns the async service and a private single-threaded tokio
//! runtime. Do not use these from inside an async runtime.
//!
//! ```no_run
//! use ogc_client::blocking::WebMapService;
//! use ogc_client::wms::WmsVersion;
//!
//! let wms = WebMapService::connect("https://ows.terrestris.de/osm/service", WmsVersion::V1_1_1)?;
//! println!("{:?}", wms.capabilities().service.title);
//! # Ok::<(), ogc_client::Error>(())
//! ```

use std::future::Future;
use std::sync::Arc;

use tokio::runtime::Runtime;

use crate::Result;
use crate::csw::{self, ElementSet, GetRecords, RecordsResponse};
use crate::http::Client;
use crate::ogcapi::{self, ItemsPage, ItemsQuery};
use crate::wfs::{self, FeatureResponse, FeatureSchema, GetFeature, WfsVersion};
use crate::wms::{self, GetFeatureInfo, GetMap, WmsResponse, WmsVersion};
use crate::wmts::{self, GetTile};

fn runtime() -> Result<Arc<Runtime>> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map(Arc::new)
        .map_err(|e| crate::Error::InvalidRequest(format!("cannot start runtime: {e}")))
}

fn block<F: Future>(rt: &Runtime, f: F) -> F::Output {
    rt.block_on(f)
}

macro_rules! wrapper {
    ($(#[$m:meta])* $name:ident, $inner:ty) => {
        $(#[$m])*
        #[derive(Debug, Clone)]
        pub struct $name {
            inner: $inner,
            rt: Arc<Runtime>,
        }

        impl $name {
            /// The wrapped async service.
            pub fn inner(&self) -> &$inner {
                &self.inner
            }
        }
    };
}

wrapper!(
    /// Blocking WMS client.
    WebMapService,
    wms::WebMapService
);

impl WebMapService {
    /// Connects with a default client.
    pub fn connect(url: &str, version: WmsVersion) -> Result<Self> {
        Self::connect_with(Client::new(), url, version)
    }
    /// Connects with the given client.
    pub fn connect_with(client: Client, url: &str, version: WmsVersion) -> Result<Self> {
        let rt = runtime()?;
        let inner = block(&rt, wms::WebMapService::connect_with(client, url, version))?;
        Ok(Self { inner, rt })
    }
    /// Capabilities.
    pub fn capabilities(&self) -> &wms::WmsCapabilities {
        self.inner.capabilities()
    }
    /// GetMap.
    pub fn get_map(&self, req: &GetMap) -> Result<WmsResponse> {
        block(&self.rt, self.inner.get_map(req))
    }
    /// GetFeatureInfo.
    pub fn get_feature_info(&self, req: &GetFeatureInfo) -> Result<WmsResponse> {
        block(&self.rt, self.inner.get_feature_info(req))
    }
}

wrapper!(
    /// Blocking WMTS client.
    WebMapTileService,
    wmts::WebMapTileService
);

impl WebMapTileService {
    /// Connects with a default client.
    pub fn connect(url: &str) -> Result<Self> {
        Self::connect_with(Client::new(), url)
    }
    /// Connects with the given client.
    pub fn connect_with(client: Client, url: &str) -> Result<Self> {
        let rt = runtime()?;
        let inner = block(&rt, wmts::WebMapTileService::connect_with(client, url))?;
        Ok(Self { inner, rt })
    }
    /// Capabilities.
    pub fn capabilities(&self) -> &wmts::WmtsCapabilities {
        self.inner.capabilities()
    }
    /// GetTile.
    pub fn get_tile(&self, req: &GetTile) -> Result<WmsResponse> {
        block(&self.rt, self.inner.get_tile(req))
    }
}

wrapper!(
    /// Blocking WFS client.
    WebFeatureService,
    wfs::WebFeatureService
);

impl WebFeatureService {
    /// Connects with a default client.
    pub fn connect(url: &str, version: WfsVersion) -> Result<Self> {
        Self::connect_with(Client::new(), url, version)
    }
    /// Connects with the given client.
    pub fn connect_with(client: Client, url: &str, version: WfsVersion) -> Result<Self> {
        let rt = runtime()?;
        let inner = block(
            &rt,
            wfs::WebFeatureService::connect_with(client, url, version),
        )?;
        Ok(Self { inner, rt })
    }
    /// Capabilities.
    pub fn capabilities(&self) -> &wfs::WfsCapabilities {
        self.inner.capabilities()
    }
    /// GetFeature.
    pub fn get_feature(&self, req: &GetFeature) -> Result<FeatureResponse> {
        block(&self.rt, self.inner.get_feature(req))
    }
    /// Hit count.
    pub fn hits(&self, req: &GetFeature) -> Result<Option<u64>> {
        block(&self.rt, self.inner.hits(req))
    }
    /// DescribeFeatureType.
    pub fn describe_feature_type(&self, type_name: &str) -> Result<FeatureSchema> {
        block(&self.rt, self.inner.describe_feature_type(type_name))
    }
}

wrapper!(
    /// Blocking CSW client.
    CatalogueService,
    csw::CatalogueService
);

impl CatalogueService {
    /// Connects with a default client.
    pub fn connect(url: &str) -> Result<Self> {
        Self::connect_with(Client::new(), url)
    }
    /// Connects with the given client.
    pub fn connect_with(client: Client, url: &str) -> Result<Self> {
        let rt = runtime()?;
        let inner = block(&rt, csw::CatalogueService::connect_with(client, url))?;
        Ok(Self { inner, rt })
    }
    /// Capabilities.
    pub fn capabilities(&self) -> &csw::CswCapabilities {
        self.inner.capabilities()
    }
    /// GetRecords.
    pub fn get_records(&self, req: &GetRecords) -> Result<RecordsResponse> {
        block(&self.rt, self.inner.get_records(req))
    }
    /// GetRecordById.
    pub fn get_record_by_id(
        &self,
        ids: &[&str],
        element_set: ElementSet,
        output_schema: Option<&str>,
    ) -> Result<RecordsResponse> {
        block(
            &self.rt,
            self.inner.get_record_by_id(ids, element_set, output_schema),
        )
    }
}

wrapper!(
    /// Blocking OGC API – Features / Records client.
    Features,
    ogcapi::Features
);

impl Features {
    /// A client for the API rooted at `url`.
    pub fn new(url: &str) -> Result<Self> {
        Self::with_client(Client::new(), url)
    }
    /// With a custom HTTP client.
    pub fn with_client(client: Client, url: &str) -> Result<Self> {
        Ok(Self {
            inner: ogcapi::Features::with_client(client, url)?,
            rt: runtime()?,
        })
    }
    /// Collections.
    pub fn collections(&self) -> Result<ogcapi::Collections> {
        block(&self.rt, self.inner.collections())
    }
    /// One page of items.
    pub fn items(&self, collection: &str, query: &ItemsQuery) -> Result<ItemsPage> {
        block(&self.rt, self.inner.items(collection, query))
    }
    /// Next page.
    pub fn next_page(&self, page: &ItemsPage) -> Result<Option<ItemsPage>> {
        block(&self.rt, self.inner.next_page(page))
    }
    /// One item.
    pub fn item(&self, collection: &str, id: &str) -> Result<geojson::Feature> {
        block(&self.rt, self.inner.item(collection, id))
    }
}
