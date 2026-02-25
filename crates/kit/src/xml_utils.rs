//! XML utilities using quick-xml for efficient XML generation and parsing
//!
//! This module provides helper functions for generating and parsing XML using the quick-xml crate,
//! replacing string-based XML manipulation with proper XML handling.

// On non-Linux, much of this module is unused as it's primarily for libvirt XML
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use color_eyre::{eyre::eyre, Result};
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::reader::Reader;
use quick_xml::writer::Writer;
use std::collections::HashMap;
use std::io::Cursor;

/// A builder for creating XML documents with quick-xml
pub struct XmlWriter {
    writer: Writer<Cursor<Vec<u8>>>,
}

impl std::fmt::Debug for XmlWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("XmlWriter").finish_non_exhaustive()
    }
}

impl XmlWriter {
    /// Create a new XML writer
    pub fn new() -> Self {
        Self {
            writer: Writer::new(Cursor::new(Vec::new())),
        }
    }

    /// Start an XML element with attributes
    pub fn start_element(&mut self, name: &str, attributes: &[(&str, &str)]) -> Result<()> {
        let mut elem = BytesStart::new(name);
        for (key, value) in attributes {
            elem.push_attribute((*key, *value));
        }
        self.writer
            .write_event(Event::Start(elem))
            .map_err(|e| eyre!("Failed to write start element: {}", e))?;
        Ok(())
    }

    /// Write a simple element with text content
    pub fn write_text_element(&mut self, name: &str, text: &str) -> Result<()> {
        self.start_element(name, &[])?;
        self.write_text(text)?;
        self.end_element(name)?;
        Ok(())
    }

    /// Write a simple element with text content and attributes
    pub fn write_text_element_with_attrs(
        &mut self,
        name: &str,
        text: &str,
        attributes: &[(&str, &str)],
    ) -> Result<()> {
        self.start_element(name, attributes)?;
        if !text.is_empty() {
            self.write_text(text)?;
        }
        self.end_element(name)?;
        Ok(())
    }

    /// Write a self-closing element with attributes
    pub fn write_empty_element(&mut self, name: &str, attributes: &[(&str, &str)]) -> Result<()> {
        let mut elem = BytesStart::new(name);
        for (key, value) in attributes {
            elem.push_attribute((*key, *value));
        }
        self.writer
            .write_event(Event::Empty(elem))
            .map_err(|e| eyre!("Failed to write empty element: {}", e))?;
        Ok(())
    }

    /// Write text content
    pub fn write_text(&mut self, text: &str) -> Result<()> {
        if !text.is_empty() {
            self.writer
                .write_event(Event::Text(BytesText::new(text)))
                .map_err(|e| eyre!("Failed to write text: {}", e))?;
        }
        Ok(())
    }

    /// End an XML element
    pub fn end_element(&mut self, name: &str) -> Result<()> {
        self.writer
            .write_event(Event::End(BytesEnd::new(name)))
            .map_err(|e| eyre!("Failed to write end element: {}", e))?;
        Ok(())
    }

    /// Get the generated XML as a string
    pub fn into_string(self) -> Result<String> {
        let bytes = self.writer.into_inner().into_inner();
        String::from_utf8(bytes).map_err(|e| eyre!("Failed to convert XML to string: {}", e))
    }
}

impl Default for XmlWriter {
    fn default() -> Self {
        Self::new()
    }
}

/// Simple DOM node for XML parsing
#[derive(Debug, Clone)]
pub struct XmlNode {
    /// Element name (tag name)
    pub name: String,
    /// Element attributes as key-value pairs
    pub attributes: HashMap<String, String>,
    /// Text content of the element
    pub text: String,
    /// Child elements
    pub children: Vec<XmlNode>,
}

impl XmlNode {
    /// Find first element by name (recursive search)
    pub fn find(&self, element_name: &str) -> Option<&XmlNode> {
        if self.name == element_name {
            return Some(self);
        }

        for child in &self.children {
            if let Some(found) = child.find(element_name) {
                return Some(found);
            }
        }

        None
    }

    /// Find first element by name with namespace fallback
    pub fn find_with_namespace(&self, element_name: &str) -> Option<&XmlNode> {
        // Try namespaced version first
        if let Some(found) = self.find(&format!("bootc:{}", element_name)) {
            return Some(found);
        }
        // Fallback to non-namespaced
        self.find(element_name)
    }

    /// Get text content of this node
    pub fn text_content(&self) -> &str {
        &self.text
    }
}

/// Parse XML string into a simple DOM structure
pub fn parse_xml_dom(xml: &str) -> Result<XmlNode> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();

    let mut stack: Vec<XmlNode> = Vec::new();
    let mut root: Option<XmlNode> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                let mut attributes = HashMap::new();

                for attr in e.attributes() {
                    if let Ok(attr) = attr {
                        let key = String::from_utf8_lossy(attr.key.as_ref()).into_owned();
                        let value = String::from_utf8_lossy(&attr.value).into_owned();
                        attributes.insert(key, value);
                    }
                }

                let node = XmlNode {
                    name,
                    attributes,
                    text: String::new(),
                    children: Vec::new(),
                };

                stack.push(node);
            }
            Ok(Event::Empty(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                let mut attributes = HashMap::new();

                for attr in e.attributes() {
                    if let Ok(attr) = attr {
                        let key = String::from_utf8_lossy(attr.key.as_ref()).into_owned();
                        let value = String::from_utf8_lossy(&attr.value).into_owned();
                        attributes.insert(key, value);
                    }
                }

                let node = XmlNode {
                    name,
                    attributes,
                    text: String::new(),
                    children: Vec::new(),
                };

                // Add to parent or set as root
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(node);
                } else if root.is_none() {
                    root = Some(node);
                }
            }
            Ok(Event::End(_)) => {
                if let Some(completed_node) = stack.pop() {
                    if let Some(parent) = stack.last_mut() {
                        parent.children.push(completed_node);
                    } else {
                        root = Some(completed_node);
                    }
                }
            }
            Ok(Event::Text(e)) => {
                if let Ok(text) = e.unescape() {
                    if let Some(current) = stack.last_mut() {
                        current.text.push_str(&text);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(eyre!("Failed to parse XML: {}", e)),
            _ => {}
        }
        buf.clear();
    }

    root.ok_or_else(|| eyre!("No root element found in XML"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xml_writer_basic() {
        let mut writer = XmlWriter::new();
        writer.start_element("root", &[]).unwrap();
        writer.write_text_element("name", "test").unwrap();
        writer
            .write_text_element_with_attrs("memory", "4096", &[("unit", "MiB")])
            .unwrap();
        writer
            .write_empty_element("disk", &[("type", "file")])
            .unwrap();
        writer.end_element("root").unwrap();

        let xml = writer.into_string().unwrap();
        assert!(xml.contains("<root>"));
        assert!(xml.contains("<name>test</name>"));
        assert!(xml.contains("<memory unit=\"MiB\">4096</memory>"));
        assert!(xml.contains("<disk type=\"file\"/>"));
        assert!(xml.contains("</root>"));
    }

    #[test]
    fn test_find_with_namespace() {
        let xml = r#"
            <domain>
                <metadata>
                    <bootc:container xmlns:bootc="https://github.com/containers/bootc">
                        <bootc:source-image>quay.io/fedora/fedora-bootc:42</bootc:source-image>
                        <bootc:filesystem>xfs</bootc:filesystem>
                    </bootc:container>
                </metadata>
            </domain>
        "#;

        let dom = parse_xml_dom(xml).unwrap();

        assert_eq!(
            dom.find_with_namespace("source-image")
                .map(|n| n.text_content().to_string()),
            Some("quay.io/fedora/fedora-bootc:42".to_string())
        );
        assert_eq!(
            dom.find_with_namespace("filesystem")
                .map(|n| n.text_content().to_string()),
            Some("xfs".to_string())
        );
        assert_eq!(
            dom.find_with_namespace("nonexistent")
                .map(|n| n.text_content().to_string()),
            None
        );
    }

    #[test]
    fn test_xml_writer_complex() {
        let mut writer = XmlWriter::new();
        writer.start_element("domain", &[("type", "kvm")]).unwrap();
        writer.write_text_element("name", "test-domain").unwrap();
        writer
            .write_text_element_with_attrs("memory", "4096", &[("unit", "MiB")])
            .unwrap();

        // Test nested elements
        writer.start_element("devices", &[]).unwrap();
        writer
            .write_empty_element("disk", &[("type", "file"), ("device", "disk")])
            .unwrap();
        writer
            .start_element("interface", &[("type", "network")])
            .unwrap();
        writer
            .write_empty_element("source", &[("network", "default")])
            .unwrap();
        writer.end_element("interface").unwrap();
        writer.end_element("devices").unwrap();

        writer.end_element("domain").unwrap();

        let xml = writer.into_string().unwrap();
        assert!(xml.contains("<domain type=\"kvm\">"));
        assert!(xml.contains("<devices>"));
        assert!(xml.contains("<disk type=\"file\" device=\"disk\"/>"));
        assert!(xml.contains("<interface type=\"network\">"));
        assert!(xml.contains("<source network=\"default\"/>"));
        assert!(xml.contains("</interface>"));
        assert!(xml.contains("</devices>"));
        assert!(xml.contains("</domain>"));
    }

    #[test]
    fn test_xml_writer_empty_text() {
        let mut writer = XmlWriter::new();
        writer.start_element("root", &[]).unwrap();
        writer.write_text_element("empty", "").unwrap();
        writer
            .write_text_element_with_attrs("empty-with-attrs", "", &[("type", "test")])
            .unwrap();
        writer.end_element("root").unwrap();

        let xml = writer.into_string().unwrap();
        assert!(xml.contains("<empty></empty>"));
        assert!(xml.contains("<empty-with-attrs type=\"test\"></empty-with-attrs>"));
    }

    #[test]
    fn test_find_with_namespace_edge_cases() {
        // Test with both namespaced and non-namespaced elements
        let xml = r#"
            <domain>
                <metadata>
                    <bootc:container xmlns:bootc="https://github.com/containers/bootc">
                        <bootc:source-image>namespaced-image</bootc:source-image>
                        <source-image>non-namespaced-image</source-image>
                    </bootc:container>
                </metadata>
            </domain>
        "#;

        let dom = parse_xml_dom(xml).unwrap();

        // Should find the namespaced version first
        assert_eq!(
            dom.find_with_namespace("source-image")
                .map(|n| n.text_content().to_string()),
            Some("namespaced-image".to_string())
        );
    }

    #[test]
    fn test_xml_writer_nested_elements() {
        let mut writer = XmlWriter::new();
        writer.start_element("root", &[]).unwrap();
        writer.write_text_element("custom", "raw content").unwrap();
        writer.end_element("root").unwrap();

        let xml = writer.into_string().unwrap();
        assert!(xml.contains("<root>"));
        assert!(xml.contains("<custom>raw content</custom>"));
        assert!(xml.contains("</root>"));
    }
}
