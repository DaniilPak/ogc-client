//! HTTP layer: a thin, injectable wrapper around `reqwest::Client`.

use std::time::Duration;

use bytes::Bytes;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use url::Url;

use crate::error::{Error, Result};

/// Authentication applied to every request.
#[derive(Clone, Debug, Default)]
pub enum Auth {
    /// No authentication.
    #[default]
    None,
    /// HTTP basic authentication.
    Basic {
        /// User name.
        username: String,
        /// Password.
        password: Option<String>,
    },
    /// `Authorization: Bearer <token>`.
    Bearer(String),
}

/// HTTP client used by all services. Cheap to clone.
///
/// Inject your own `reqwest::Client` (proxies, custom TLS roots, middleware)
/// with [`Client::from_reqwest`].
#[derive(Clone, Debug)]
pub struct Client {
    inner: reqwest::Client,
    auth: Auth,
    headers: HeaderMap,
    timeout: Option<Duration>,
}

impl Default for Client {
    fn default() -> Self {
        Client::new()
    }
}

/// A raw HTTP response.
#[derive(Clone, Debug)]
pub struct Response {
    /// Final URL (after redirects).
    pub url: Url,
    /// Status code.
    pub status: u16,
    /// `Content-Type` header, if present.
    pub content_type: Option<String>,
    /// Response body.
    pub body: Bytes,
}

impl Response {
    /// Whether the content type mentions XML.
    pub fn is_xml(&self) -> bool {
        self.content_type
            .as_deref()
            .map(|c| c.contains("xml"))
            .unwrap_or(false)
    }

    /// Whether the content type mentions JSON.
    pub fn is_json(&self) -> bool {
        self.content_type
            .as_deref()
            .map(|c| c.contains("json"))
            .unwrap_or(false)
    }
}

impl Client {
    /// A client with a default `reqwest::Client` (rustls, 30 s timeout).
    pub fn new() -> Client {
        let inner = reqwest::Client::builder()
            .user_agent(concat!("ogc-client/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default();
        Client {
            inner,
            auth: Auth::None,
            headers: HeaderMap::new(),
            timeout: Some(Duration::from_secs(30)),
        }
    }

    /// Wraps an existing `reqwest::Client`.
    pub fn from_reqwest(inner: reqwest::Client) -> Client {
        Client {
            inner,
            auth: Auth::None,
            headers: HeaderMap::new(),
            timeout: None,
        }
    }

    /// Sets authentication.
    pub fn with_auth(mut self, auth: Auth) -> Client {
        self.auth = auth;
        self
    }

    /// Adds a header sent with every request.
    pub fn with_header(mut self, name: &str, value: &str) -> Result<Client> {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|e| Error::InvalidRequest(e.to_string()))?;
        let value =
            HeaderValue::from_str(value).map_err(|e| Error::InvalidRequest(e.to_string()))?;
        self.headers.insert(name, value);
        Ok(self)
    }

    /// Sets the per-request timeout (`None` disables it).
    pub fn with_timeout(mut self, timeout: Option<Duration>) -> Client {
        self.timeout = timeout;
        self
    }

    /// The wrapped `reqwest::Client`.
    pub fn reqwest(&self) -> &reqwest::Client {
        &self.inner
    }

    fn prepare(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let mut rb = rb.headers(self.headers.clone());
        if let Some(t) = self.timeout {
            rb = rb.timeout(t);
        }
        match &self.auth {
            Auth::None => rb,
            Auth::Basic { username, password } => rb.basic_auth(username, password.as_ref()),
            Auth::Bearer(t) => rb.bearer_auth(t),
        }
    }

    /// GET `url` with an `Accept` header. Non-2xx statuses become errors.
    pub async fn get(&self, url: &Url, accept: Option<&str>) -> Result<Response> {
        let mut rb = self.inner.get(url.clone());
        if let Some(a) = accept {
            rb = rb.header(ACCEPT, a);
        }
        self.send(self.prepare(rb), url).await
    }

    /// POST `body` with the given content type.
    pub async fn post(&self, url: &Url, body: String, content_type: &str) -> Result<Response> {
        let rb = self
            .inner
            .post(url.clone())
            .header(CONTENT_TYPE, content_type)
            .body(body);
        self.send(self.prepare(rb), url).await
    }

    async fn send(&self, rb: reqwest::RequestBuilder, url: &Url) -> Result<Response> {
        let resp = rb.send().await?;
        let status = resp.status().as_u16();
        let final_url = resp.url().clone();
        let content_type = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let body = resp.bytes().await?;
        if !(200..300).contains(&status) {
            // OGC servers often wrap errors in an exception report.
            if let Some(err) = crate::error::sniff_exception(&body) {
                return Err(err);
            }
            if let Some(err) = crate::ogcapi::api_error(&body) {
                return Err(err);
            }
            let text = String::from_utf8_lossy(&body[..body.len().min(500)]).into_owned();
            return Err(Error::Status {
                status,
                url: url.to_string(),
                body: text,
            });
        }
        Ok(Response {
            url: final_url,
            status,
            content_type,
            body,
        })
    }
}

/// Builds a URL from a base and KVP parameters, replacing any existing
/// parameters with the same (case-insensitive) name and keeping the others
/// (vendor parameters such as `map=` on MapServer).
pub fn build_url(base: &str, params: &[(String, String)]) -> Result<Url> {
    let mut url = Url::parse(base.trim())?;
    let keep: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| !params.iter().any(|(p, _)| p.eq_ignore_ascii_case(k)))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    {
        let mut q = url.query_pairs_mut();
        q.clear();
        for (k, v) in keep.iter().chain(params.iter()) {
            q.append_pair(k, v);
        }
    }
    if url.query() == Some("") {
        url.set_query(None);
    }
    Ok(url)
}

/// Strips service KVP parameters (`service`, `request`, `version`) from a URL,
/// keeping vendor parameters.
pub(crate) fn clean_service_url(base: &str) -> Result<Url> {
    let mut url = Url::parse(base.trim())?;
    let keep: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| {
            !["service", "request", "version", "acceptversions"]
                .iter()
                .any(|p| p.eq_ignore_ascii_case(k))
        })
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if keep.is_empty() {
        url.set_query(None);
    } else {
        url.query_pairs_mut().clear().extend_pairs(keep);
    }
    Ok(url)
}

/// Helper to build KVP parameter lists.
#[derive(Debug, Default, Clone)]
pub(crate) struct Kvp(pub Vec<(String, String)>);

impl Kvp {
    pub fn new() -> Kvp {
        Kvp(Vec::new())
    }
    pub fn push(&mut self, k: &str, v: impl Into<String>) -> &mut Kvp {
        self.0.push((k.to_string(), v.into()));
        self
    }
    pub fn opt(&mut self, k: &str, v: Option<impl Into<String>>) -> &mut Kvp {
        if let Some(v) = v {
            self.0.push((k.to_string(), v.into()));
        }
        self
    }
    pub fn extend(&mut self, extra: &[(String, String)]) -> &mut Kvp {
        for (k, v) in extra {
            self.0.retain(|(e, _)| !e.eq_ignore_ascii_case(k));
            self.0.push((k.clone(), v.clone()));
        }
        self
    }
}

/// Formats a number without a trailing `.0` for integral values.
pub(crate) fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_url_keeps_vendor_params() {
        let u = build_url(
            "https://example.com/cgi?map=/x.map&SERVICE=WMS",
            &[
                ("service".into(), "WMS".into()),
                ("request".into(), "GetMap".into()),
            ],
        )
        .unwrap();
        assert_eq!(
            u.as_str(),
            "https://example.com/cgi?map=%2Fx.map&service=WMS&request=GetMap"
        );
        let c = clean_service_url("https://e.com/wms?request=GetCapabilities&map=a").unwrap();
        assert_eq!(c.as_str(), "https://e.com/wms?map=a");
        assert_eq!(fmt_num(10.0), "10");
        assert_eq!(fmt_num(-0.5), "-0.5");
    }
}
