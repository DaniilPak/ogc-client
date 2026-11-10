//! A small, namespace-aware XML element tree built on `quick-xml`.
//!
//! OGC capabilities documents mix many namespaces and prefixes, and real
//! servers are inconsistent about which prefix they bind. Matching on local
//! names over a tree (as OWSLib does with ElementTree) is far more robust than
//! fixed serde mappings, so the parsers in this crate walk this tree.

use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::Event;
use quick_xml::name::ResolveResult;
use quick_xml::{NsReader, XmlVersion};

use crate::error::{Error, Result};

/// An XML element with its namespace, attributes, text and child elements.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Element {
    /// Namespace URI, if the element is bound to one.
    pub namespace: Option<String>,
    /// Local name (without prefix).
    pub name: String,
    /// Prefix used in the source document, if any.
    pub prefix: Option<String>,
    /// Attributes as `(qualified name, value)` pairs, in document order.
    pub attributes: Vec<(String, String)>,
    /// Child elements in document order.
    pub children: Vec<Element>,
    /// Concatenated character data directly inside this element.
    pub text: String,
}

impl Element {
    /// Parses a whole document and returns its root element.
    pub fn parse(bytes: &[u8]) -> Result<Element> {
        let mut reader = NsReader::from_reader(bytes);
        reader.config_mut().expand_empty_elements = true;
        let mut buf = Vec::new();
        let mut stack: Vec<Element> = Vec::new();
        let mut root: Option<Element> = None;
        loop {
            let (ns, event) = reader
                .read_resolved_event_into(&mut buf)
                .map_err(|e| Error::Xml(e.to_string()))?;
            match event {
                Event::Start(start) => {
                    let namespace = match ns {
                        ResolveResult::Bound(n) => Some(n.as_ref().to_string()),
                        _ => None,
                    };
                    let qname = start.name();
                    let name = qname.local_name().as_ref().to_string();
                    let prefix = qname.prefix().map(|p| p.as_ref().to_string());
                    let mut attributes = Vec::new();
                    for attr in start.attributes().with_checks(false) {
                        let attr = attr.map_err(|e| Error::Xml(e.to_string()))?;
                        let key = attr.key.as_ref().to_string();
                        let value = attr
                            .normalized_value(XmlVersion::Implicit1_0)
                            .map(|v| v.into_owned())
                            .unwrap_or_else(|_| attr.value.to_string());
                        attributes.push((key, value));
                    }
                    stack.push(Element {
                        namespace,
                        name,
                        prefix,
                        attributes,
                        children: Vec::new(),
                        text: String::new(),
                    });
                }
                Event::End(_) => {
                    let el = stack
                        .pop()
                        .ok_or_else(|| Error::Xml("unbalanced end tag".into()))?;
                    match stack.last_mut() {
                        Some(parent) => parent.children.push(el),
                        None => {
                            root = Some(el);
                        }
                    }
                }
                Event::Text(t) => {
                    if let Some(cur) = stack.last_mut() {
                        cur.text.push_str(&t.xml10_content());
                    }
                }
                Event::CData(t) => {
                    if let Some(cur) = stack.last_mut() {
                        cur.text.push_str(&t.xml10_content());
                    }
                }
                Event::GeneralRef(r) => {
                    if let Some(cur) = stack.last_mut() {
                        if let Ok(Some(c)) = r.resolve_char_ref() {
                            cur.text.push(c);
                        } else {
                            let name = r.xml10_content();
                            match resolve_predefined_entity(&name) {
                                Some(s) => cur.text.push_str(s),
                                None => {
                                    cur.text.push('&');
                                    cur.text.push_str(&name);
                                    cur.text.push(';');
                                }
                            }
                        }
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
        }
        root.ok_or_else(|| Error::Xml("document has no root element".into()))
    }

    /// Whether this element has the given local name.
    pub fn is(&self, name: &str) -> bool {
        self.name == name
    }

    /// First direct child with the given local name.
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }

    /// First direct child matching any of the given local names.
    pub fn child_any(&self, names: &[&str]) -> Option<&Element> {
        self.children
            .iter()
            .find(|c| names.contains(&c.name.as_str()))
    }

    /// All direct children with the given local name.
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// Follows a path of local names through direct children.
    pub fn path(&self, path: &[&str]) -> Option<&Element> {
        let mut cur = self;
        for p in path {
            cur = cur.child(p)?;
        }
        Some(cur)
    }

    /// Trimmed text of the element at `path`, if non-empty.
    pub fn path_text(&self, path: &[&str]) -> Option<String> {
        self.path(path).and_then(Element::text_opt)
    }

    /// Trimmed text of a direct child, if non-empty.
    pub fn child_text(&self, name: &str) -> Option<String> {
        self.child(name).and_then(Element::text_opt)
    }

    /// Trimmed texts of all direct children with the given name.
    pub fn children_text(&self, name: &str) -> Vec<String> {
        self.children_named(name)
            .filter_map(Element::text_opt)
            .collect()
    }

    /// Trimmed text, or `None` if empty.
    pub fn text_opt(&self) -> Option<String> {
        let t = self.text.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    }

    /// Trimmed text (possibly empty).
    pub fn text_trim(&self) -> &str {
        self.text.trim()
    }

    /// Attribute value by local name (ignores the prefix, so `xlink:href`
    /// matches `href`).
    pub fn attr(&self, local: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(k, _)| k == local || k.rsplit(':').next() == Some(local))
            .map(|(_, v)| v.as_str())
    }

    /// Attribute parsed as a boolean (`1`/`true`).
    pub fn attr_bool(&self, local: &str) -> Option<bool> {
        self.attr(local)
            .map(|v| matches!(v.trim(), "1" | "true" | "TRUE" | "True"))
    }

    /// Attribute parsed as a number.
    pub fn attr_f64(&self, local: &str) -> Option<f64> {
        self.attr(local).and_then(|v| v.trim().parse().ok())
    }

    /// Attribute parsed as an unsigned integer.
    pub fn attr_u32(&self, local: &str) -> Option<u32> {
        self.attr(local).and_then(|v| v.trim().parse().ok())
    }

    /// Depth-first iterator over this element and all descendants.
    pub fn descendants(&self) -> Descendants<'_> {
        Descendants { stack: vec![self] }
    }

    /// First descendant (or self) with the given local name.
    pub fn find(&self, name: &str) -> Option<&Element> {
        self.descendants().find(|e| e.name == name)
    }

    /// All text in this element and its descendants, concatenated.
    pub fn deep_text(&self) -> String {
        let mut s = self.text.clone();
        for c in &self.children {
            s.push_str(&c.deep_text());
        }
        s
    }

    /// Serializes this element (and its subtree) back to an XML string.
    ///
    /// Namespace declarations are emitted from the recorded prefixes so the
    /// fragment is self-contained.
    pub fn to_xml_string(&self) -> String {
        let mut out = String::new();
        let mut declared: Vec<(Option<String>, String)> = Vec::new();
        self.write_into(&mut out, &mut declared);
        out
    }

    fn write_into(&self, out: &mut String, declared: &mut Vec<(Option<String>, String)>) {
        let qname = match &self.prefix {
            Some(p) => format!("{p}:{}", self.name),
            None => self.name.clone(),
        };
        out.push('<');
        out.push_str(&qname);
        let mark = declared.len();
        if let Some(ns) = &self.namespace {
            let already = declared
                .iter()
                .rev()
                .find(|(p, _)| *p == self.prefix)
                .map(|(_, n)| n == ns)
                .unwrap_or(false);
            let has_attr = self.attributes.iter().any(|(k, _)| match &self.prefix {
                Some(p) => k == &format!("xmlns:{p}"),
                None => k == "xmlns",
            });
            if !already && !has_attr {
                match &self.prefix {
                    Some(p) => out.push_str(&format!(" xmlns:{p}=\"{}\"", escape(ns))),
                    None => out.push_str(&format!(" xmlns=\"{}\"", escape(ns))),
                }
            }
            declared.push((self.prefix.clone(), ns.clone()));
        }
        for (k, v) in &self.attributes {
            out.push_str(&format!(" {k}=\"{}\"", escape(v)));
        }
        if self.children.is_empty() && self.text.is_empty() {
            out.push_str("/>");
        } else {
            out.push('>');
            out.push_str(&escape(&self.text));
            for c in &self.children {
                c.write_into(out, declared);
            }
            out.push_str(&format!("</{qname}>"));
        }
        declared.truncate(mark);
    }
}

/// Depth-first iterator returned by [`Element::descendants`].
pub struct Descendants<'a> {
    stack: Vec<&'a Element>,
}

impl<'a> Iterator for Descendants<'a> {
    type Item = &'a Element;
    fn next(&mut self) -> Option<&'a Element> {
        let e = self.stack.pop()?;
        for c in e.children.iter().rev() {
            self.stack.push(c);
        }
        Some(e)
    }
}

/// Escapes text for use in XML content or attribute values.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// Parses a whitespace-separated list of numbers.
pub(crate) fn parse_numbers(s: &str) -> Vec<f64> {
    s.split(|c: char| c.is_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
        .filter_map(|t| t.parse().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_namespaces_entities_and_attributes() {
        let xml = br#"<?xml version="1.0"?>
<a:root xmlns:a="urn:a" xmlns:xlink="http://www.w3.org/1999/xlink">
  <a:child xlink:href="http://x?a=1&amp;b=2">Tom &amp; Jerry &#65;</a:child>
  <other/>
</a:root>"#;
        let root = Element::parse(xml).unwrap();
        assert_eq!(root.name, "root");
        assert_eq!(root.namespace.as_deref(), Some("urn:a"));
        let child = root.child("child").unwrap();
        assert_eq!(child.attr("href"), Some("http://x?a=1&b=2"));
        assert_eq!(child.text_trim(), "Tom & Jerry A");
        assert!(root.child("other").is_some());
        let round = Element::parse(root.to_xml_string().as_bytes()).unwrap();
        assert_eq!(round.child("child").unwrap().text_trim(), "Tom & Jerry A");
    }

    #[test]
    fn numbers() {
        assert_eq!(parse_numbers(" 1 2.5\n-3,4 "), vec![1.0, 2.5, -3.0, 4.0]);
    }
}
