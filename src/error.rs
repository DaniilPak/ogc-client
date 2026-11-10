//! Error type shared by all services.

use crate::xml::Element;

/// Result alias using [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A single exception reported by an OGC service.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ServiceException {
    /// `exceptionCode` / `code` attribute, e.g. `InvalidParameterValue`.
    pub code: Option<String>,
    /// `locator` attribute, e.g. the offending parameter name.
    pub locator: Option<String>,
    /// Human-readable exception text.
    pub text: String,
}

impl std::fmt::Display for ServiceException {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(code) = &self.code {
            write!(f, "[{code}] ")?;
        }
        if let Some(loc) = &self.locator {
            write!(f, "(locator: {loc}) ")?;
        }
        f.write_str(self.text.trim())
    }
}

/// Errors returned by this crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Transport-level HTTP error (DNS, TLS, connection, timeout...).
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    /// The server answered with a non-success status code.
    #[error("HTTP status {status} from {url}: {body}")]
    Status {
        /// Status code.
        status: u16,
        /// Requested URL.
        url: String,
        /// Start of the response body (truncated).
        body: String,
    },
    /// The server returned an OGC exception report.
    #[error("service exception: {}", .0.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; "))]
    Service(Vec<ServiceException>),
    /// An OGC API endpoint returned a JSON problem/exception document.
    #[error("OGC API error {code}: {description}")]
    Api {
        /// `code` or `title` field.
        code: String,
        /// `description` or `detail` field.
        description: String,
    },
    /// Malformed XML.
    #[error("XML error: {0}")]
    Xml(String),
    /// Malformed JSON.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// Invalid URL.
    #[error("invalid URL: {0}")]
    Url(#[from] url::ParseError),
    /// The document parsed but did not have the expected structure.
    #[error("unexpected document: {0}")]
    Parse(String),
    /// The requested operation, format or option is not offered by the service.
    #[error("not supported: {0}")]
    Unsupported(String),
    /// Invalid arguments supplied by the caller.
    #[error("invalid request: {0}")]
    InvalidRequest(String),
}

/// Returns `Some(Error::Service)` if the element is an OGC exception report
/// (`ows:ExceptionReport` or WMS `ServiceExceptionReport`).
pub(crate) fn exception_from_element(root: &Element) -> Option<Error> {
    match root.name.as_str() {
        "ExceptionReport" => {
            let list = root
                .descendants()
                .filter(|e| e.is("Exception"))
                .map(|e| ServiceException {
                    code: e.attr("exceptionCode").map(str::to_string),
                    locator: e.attr("locator").map(str::to_string),
                    text: e
                        .children_named("ExceptionText")
                        .map(|t| t.text_trim().to_string())
                        .collect::<Vec<_>>()
                        .join("\n"),
                })
                .collect::<Vec<_>>();
            Some(Error::Service(list))
        }
        "ServiceExceptionReport" => {
            let list = root
                .children_named("ServiceException")
                .map(|e| ServiceException {
                    code: e.attr("code").map(str::to_string),
                    locator: e.attr("locator").map(str::to_string),
                    text: e.text_trim().to_string(),
                })
                .collect::<Vec<_>>();
            Some(Error::Service(list))
        }
        _ => None,
    }
}

/// Parses `bytes` as XML and turns an exception report into an error.
pub(crate) fn parse_xml_checked(bytes: &[u8]) -> Result<Element> {
    let root = Element::parse(bytes)?;
    if let Some(err) = exception_from_element(&root) {
        return Err(err);
    }
    Ok(root)
}

/// If `bytes` look like an XML exception report, returns it as an error.
pub(crate) fn sniff_exception(bytes: &[u8]) -> Option<Error> {
    let head = &bytes[..bytes.len().min(2048)];
    let head = String::from_utf8_lossy(head);
    if !(head.contains("ExceptionReport") || head.contains("ServiceException")) {
        return None;
    }
    Element::parse(bytes)
        .ok()
        .and_then(|r| exception_from_element(&r))
}
