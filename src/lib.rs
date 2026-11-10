//! # ogc-client
//!
//! Client-side access to OGC web services, in the spirit of Python's
//! [OWSLib](https://github.com/geopython/OWSLib):
//!
//! | Service | Module | Versions |
//! |---|---|---|
//! | Web Map Service | [`wms`] | 1.1.1, 1.3.0 |
//! | Web Map Tile Service | [`wmts`] | 1.0.0 (KVP + REST) |
//! | Web Feature Service | [`wfs`] | 1.1.0, 2.0.0 |
//! | Catalogue Service | [`csw`] | 2.0.2 |
//! | OGC API – Features / Records / Tiles | [`ogcapi`] | 1.0 |
//!
//! Every service offers:
//!
//! * typed capabilities parsing that works offline (`*Capabilities::parse(bytes)`),
//! * request builders that produce URLs without network access (`*_url` methods),
//! * async execution over `reqwest` with an injectable [`Client`],
//! * OGC exception reports mapped to [`Error::Service`].
//!
//! Axis order is handled for you: bounding boxes passed to request builders
//! are always `(minx, miny, maxx, maxy)` = west, south, east, north, and the
//! client swaps them where the protocol requires (WMS 1.3.0, WFS 1.1/2.0
//! URNs, GML 3, WMTS corners). See [`crs`].
//!
//! ```no_run
//! # async fn run() -> ogc_client::Result<()> {
//! use ogc_client::wms::{GetMap, WebMapService, WmsVersion};
//!
//! let wms = WebMapService::connect("https://ows.terrestris.de/osm/service", WmsVersion::V1_3_0).await?;
//! for layer in wms.capabilities().named_layers() {
//!     println!("{:?}: {:?}", layer.name, layer.title);
//! }
//! let img = wms
//!     .get_map(&GetMap::new(["OSM-WMS"], (5.0, 47.0, 15.0, 55.0), "EPSG:4326", 800, 800).format("image/png"))
//!     .await?;
//! # Ok(()) }
//! ```
//!
//! With the `blocking` feature, [`blocking`] offers synchronous wrappers.
//!
//! This crate is an independent project and is not affiliated with or
//! endorsed by the OWSLib project or geopython.

#![warn(missing_docs)]

pub mod crs;
pub mod csw;
pub mod error;
pub mod fes;
pub mod gml;
pub mod http;
pub mod ogcapi;
pub mod ows;
pub mod tms;
pub mod wfs;
pub mod wms;
pub mod wmts;
pub mod xml;

#[cfg(feature = "blocking")]
pub mod blocking;

pub use error::{Error, Result, ServiceException};
pub use http::{Auth, Client};

pub use geo_types;
pub use geojson;

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
