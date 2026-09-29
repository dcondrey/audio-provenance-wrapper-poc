//! A small strict XML reader with the semantics the project parsers depend on.
//!
//! It reproduces what Python's expat-based readers give the oracle:
//! - no DTD support: a `<!DOCTYPE` is refused unless it is the one bodyless root
//!   doctype the caller allows, and entity declarations cannot exist without a DTD;
//! - only the five predefined entities and character references expand; anything
//!   else is an error;
//! - line endings normalise to `\n`, attribute whitespace to a space;
//! - depth and element counts are bounded, and parsing is iterative.
//!
//! The tree keeps `text` (character data before the first child) and each child's
//! `tail`, like `xml.etree.ElementTree`, so it can serve both readers the oracle
//! uses: `_safe.parse_xml` (all character data of an element concatenated, no
//! tails) via [`Element::text_all`], and the plain `ET.XMLParser` used for `.als`
//! (whose tree is re-serialised and hashed) via [`Element::to_xml_string`].
//!
//! Not supported, and refused or unspecified: XML namespaces (prefixed names are
//! kept literally), encodings other than UTF-8.

use crate::project::safe::is_python_space;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Element {
    pub tag: String,
    pub attrs: Vec<(String, String)>,
    pub text: String,
    pub tail: String,
    pub children: Vec<Element>,
}

impl Element {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
    }

    pub fn get_or<'a>(&'a self, name: &str, default: &'a str) -> &'a str {
        self.get(name).unwrap_or(default)
    }

    /// Direct child with `tag`.
    pub fn child(&self, tag: &str) -> Option<&Element> {
        self.children.iter().find(|child| child.tag == tag)
    }

    /// Direct children with `tag`, in order.
    pub fn children_named<'a>(&'a self, tag: &'a str) -> impl Iterator<Item = &'a Element> {
        self.children.iter().filter(move |child| child.tag == tag)
    }

    /// All character data directly inside this element, in document order.
    pub fn text_all(&self) -> String {
        let mut out = self.text.clone();
        for child in &self.children {
            out.push_str(&child.tail);
        }
        out
    }

    /// This element and every descendant, document order.
    pub fn iter(&self) -> Vec<&Element> {
        let mut out = Vec::new();
        let mut stack = vec![self];
        while let Some(element) = stack.pop() {
            out.push(element);
            for child in element.children.iter().rev() {
                stack.push(child);
            }
        }
        out
    }

    pub fn iter_tag<'a>(&'a self, tag: &str) -> Vec<&'a Element> {
        self.iter().into_iter().filter(|element| element.tag == tag).collect()
    }

    /// The ElementPath subset the parsers use: `a/b/c`, `.//a`, `.//a/b`.
    pub fn find_all(&self, path: &str) -> Vec<&Element> {
        let (descend, rest) = match path.strip_prefix(".//") {
            Some(rest) => (true, rest),
            None => (false, path),
        };
        let mut contexts: Vec<&Element> = vec![self];
        for (position, segment) in rest.split('/').enumerate() {
            let mut next = Vec::new();
            for context in &contexts {
                if descend && position == 0 {
                    next.extend(context.iter().into_iter().skip(1).filter(|element| element.tag == segment));
                } else {
                    next.extend(context.children.iter().filter(|child| child.tag == segment));
                }
            }
            contexts = next;
        }
        contexts
    }

    pub fn find(&self, path: &str) -> Option<&Element> {
        self.find_all(path).into_iter().next()
    }

    /// `ET.tostring(element)` with the default `us-ascii` encoding: the tail is
    /// included and non-ASCII characters become decimal character references.
    pub fn to_xml_string(&self) -> String {
        let mut out = String::new();
        self.serialize(&mut out);
        let mut ascii = String::with_capacity(out.len());
        for c in out.chars() {
            if c.is_ascii() {
                ascii.push(c);
            } else {
                ascii.push_str(&format!("&#{};", c as u32));
            }
        }
        ascii
    }

    fn serialize(&self, out: &mut String) {
        // Recursion is bounded by the parser's depth limit.
        out.push('<');
        out.push_str(&self.tag);
        for (name, value) in &self.attrs {
            out.push(' ');
            out.push_str(name);
            out.push_str("=\"");
            for c in value.chars() {
                match c {
                    '&' => out.push_str("&amp;"),
                    '<' => out.push_str("&lt;"),
                    '>' => out.push_str("&gt;"),
                    '"' => out.push_str("&quot;"),
                    '\r' => out.push_str("&#13;"),
                    '\n' => out.push_str("&#10;"),
                    '\t' => out.push_str("&#09;"),
                    other => out.push(other),
                }
            }
            out.push('"');
        }
        if !self.text.is_empty() || !self.children.is_empty() {
            out.push('>');
            escape_cdata(&self.text, out);
            for child in &self.children {
                child.serialize(out);
            }
            out.push_str("</");
            out.push_str(&self.tag);
            out.push('>');
        } else {
            out.push_str(" />");
        }
        escape_cdata(&self.tail, out);
    }
}

fn escape_cdata(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct XmlLimits {
    pub max_bytes: usize,
    pub max_depth: usize,
    pub max_elements: usize,
    /// The `.als` reader is `xml.etree.ElementTree`: namespace-aware (an unbound
    /// prefix is an error, a bound one renames the tag) and it honours the declared
    /// encoding. Neither is supported here, so that reader refuses any prefixed
    /// name and any declared encoding other than UTF-8 or US-ASCII.
    pub et_mode: bool,
}

/// `Err` is the refusal reason; callers wrap it in their own error type.
pub fn parse_xml(data: &[u8], allowed_doctype: Option<&str>, limits: XmlLimits) -> Result<Element, String> {
    if data.len() > limits.max_bytes {
        return Err(format!("XML is {} bytes, past {}; refusing to parse", data.len(), limits.max_bytes));
    }
    let data = data.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(data);
    let text = core::str::from_utf8(data).map_err(|error| format!("malformed XML: {error}"))?;
    Parser { text, position: 0, limits, allowed_doctype }.document()
}

struct Parser<'a> {
    text: &'a str,
    position: usize,
    limits: XmlLimits,
    allowed_doctype: Option<&'a str>,
}

fn malformed<T>(message: &str) -> Result<T, String> {
    Err(format!("malformed XML: {message}"))
}

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c == ':' || (c as u32) >= 0x80
}

fn is_name_char(c: char) -> bool {
    is_name_start(c) || c.is_ascii_digit() || c == '-' || c == '.'
}

fn is_xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r') || (c >= ' ' && c != '\u{fffe}' && c != '\u{ffff}')
}

fn is_xml_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

impl<'a> Parser<'a> {
    fn rest(&self) -> &'a str {
        self.text.get(self.position..).unwrap_or_default()
    }

    fn starts_with(&self, prefix: &str) -> bool {
        self.rest().starts_with(prefix)
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn advance(&mut self, bytes: usize) {
        self.position += bytes;
    }

    fn skip_spaces(&mut self) -> bool {
        let rest = self.rest();
        let trimmed = rest.trim_start_matches(is_xml_space);
        let skipped = rest.len() - trimmed.len();
        self.advance(skipped);
        skipped > 0
    }

    fn name(&mut self) -> Result<&'a str, String> {
        let name = self.name_unchecked()?;
        if self.limits.et_mode && name.contains(':') {
            return malformed("XML namespaces are not supported");
        }
        Ok(name)
    }

    fn name_unchecked(&mut self) -> Result<&'a str, String> {
        let rest = self.rest();
        let mut end = 0;
        for (index, c) in rest.char_indices() {
            let ok = if index == 0 { is_name_start(c) } else { is_name_char(c) };
            if !ok {
                break;
            }
            end = index + c.len_utf8();
        }
        if end == 0 {
            return malformed("expected a name");
        }
        self.advance(end);
        rest.get(..end).map_or_else(|| malformed("bad name"), Ok)
    }

    fn document(&mut self) -> Result<Element, String> {
        if self.starts_with("<?xml") && rest_after(self.rest(), 5).is_some_and(|c| is_xml_space(c) || c == '?') {
            let end = self.rest().find("?>").ok_or("malformed XML: unterminated XML declaration")?;
            let declaration = self.rest().get(5..end).unwrap_or_default();
            if !declaration.contains("version") {
                return malformed("XML declaration has no version");
            }
            if self.limits.et_mode {
                if let Some(position) = declaration.find("encoding") {
                    let after = declaration.get(position + "encoding".len()..).unwrap_or_default();
                    let value: String = after
                        .trim_start_matches(|c: char| c == '=' || is_xml_space(c))
                        .chars()
                        .skip(1)
                        .take_while(|c| *c != '"' && *c != '\'')
                        .collect();
                    if !value.eq_ignore_ascii_case("utf-8") && !value.eq_ignore_ascii_case("us-ascii") {
                        return malformed("unsupported declared encoding");
                    }
                }
            }
            self.advance(end + 2);
        }
        let mut seen_doctype = false;
        loop {
            self.skip_spaces();
            if self.starts_with("<!--") {
                self.comment()?;
            } else if self.starts_with("<?") {
                self.processing_instruction()?;
            } else if self.starts_with("<!DOCTYPE") {
                if seen_doctype {
                    return malformed("second DOCTYPE");
                }
                seen_doctype = true;
                self.doctype()?;
            } else if self.starts_with("<") {
                break;
            } else if self.peek().is_none() {
                return Err("XML has no root element".to_owned());
            } else {
                return malformed("text outside the root element");
            }
        }
        let root = self.element_tree()?;
        loop {
            self.skip_spaces();
            if self.starts_with("<!--") {
                self.comment()?;
            } else if self.starts_with("<?") {
                self.processing_instruction()?;
            } else if self.peek().is_none() {
                return Ok(root);
            } else {
                return malformed("junk after document element");
            }
        }
    }

    fn comment(&mut self) -> Result<String, String> {
        self.advance(4);
        let rest = self.rest();
        let end = rest.find("--").ok_or("malformed XML: unterminated comment")?;
        if !rest.get(end..).is_some_and(|tail| tail.starts_with("-->")) {
            return malformed("'--' inside a comment");
        }
        let body = rest.get(..end).unwrap_or_default();
        if body.chars().any(|c| !is_xml_char(c)) {
            return malformed("invalid character in a comment");
        }
        self.advance(end + 3);
        Ok(body.to_owned())
    }

    fn processing_instruction(&mut self) -> Result<(), String> {
        self.advance(2);
        let target = self.name()?;
        if target.eq_ignore_ascii_case("xml") {
            return malformed("XML declaration not at the start of the document");
        }
        let rest = self.rest();
        let end = rest.find("?>").ok_or("malformed XML: unterminated processing instruction")?;
        if rest.get(..end).unwrap_or_default().chars().any(|c| !is_xml_char(c)) {
            return malformed("invalid character in a processing instruction");
        }
        self.advance(end + 2);
        Ok(())
    }

    /// Only a bodyless `<!DOCTYPE name>` naming the allowed root is accepted.
    fn doctype(&mut self) -> Result<(), String> {
        self.advance("<!DOCTYPE".len());
        if !self.skip_spaces() {
            return malformed("expected whitespace after DOCTYPE");
        }
        let name = self.name()?;
        self.skip_spaces();
        let bodyless = self.starts_with(">");
        if !bodyless || self.allowed_doctype != Some(name) {
            // Any external id, internal subset or unexpected name is a DTD.
            return Err("XML contains a DTD declaration; refusing to parse".to_owned());
        }
        self.advance(1);
        Ok(())
    }

    fn reference(&mut self, into: &mut String) -> Result<(), String> {
        // Positioned on '&'.
        let rest = self.rest();
        let end = rest.find(';').ok_or("malformed XML: unterminated reference")?;
        let body = rest.get(1..end).unwrap_or_default();
        let replacement: char = match body {
            "lt" => '<',
            "gt" => '>',
            "amp" => '&',
            "quot" => '"',
            "apos" => '\'',
            numeric if numeric.starts_with('#') => {
                let digits = numeric.get(1..).unwrap_or_default();
                let code = if let Some(hex) = digits.strip_prefix('x') {
                    u32::from_str_radix(hex, 16).ok().filter(|_| !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit()))
                } else {
                    digits.parse::<u32>().ok().filter(|_| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
                };
                match code.and_then(char::from_u32).filter(|c| is_xml_char(*c)) {
                    Some(c) => c,
                    None => return malformed("invalid character reference"),
                }
            }
            _ => return malformed("undefined entity"),
        };
        into.push(replacement);
        self.advance(end + 1);
        Ok(())
    }

    fn attribute_value(&mut self) -> Result<String, String> {
        let quote = self.peek().filter(|c| *c == '"' || *c == '\'').ok_or("malformed XML: expected a quoted attribute value")?;
        self.advance(1);
        let mut value = String::new();
        loop {
            let Some(c) = self.peek() else {
                return malformed("unterminated attribute value");
            };
            if c == quote {
                self.advance(1);
                return Ok(value);
            }
            match c {
                '<' => return malformed("'<' in an attribute value"),
                '&' => self.reference(&mut value)?,
                '\r' => {
                    self.advance(1);
                    if self.peek() == Some('\n') {
                        self.advance(1);
                    }
                    value.push(' ');
                }
                '\n' | '\t' => {
                    self.advance(1);
                    value.push(' ');
                }
                other => {
                    if !is_xml_char(other) {
                        return malformed("invalid character in an attribute value");
                    }
                    self.advance(other.len_utf8());
                    value.push(other);
                }
            }
        }
    }

    /// Character data up to the next `<`; a lone `]]>` is illegal.
    fn char_data(&mut self, into: &mut String) -> Result<(), String> {
        while let Some(c) = self.peek() {
            match c {
                '<' => break,
                '&' => self.reference(into)?,
                '\r' => {
                    self.advance(1);
                    if self.peek() == Some('\n') {
                        self.advance(1);
                    }
                    into.push('\n');
                }
                ']' if self.starts_with("]]>") => return malformed("']]>' in character data"),
                other => {
                    if !is_xml_char(other) {
                        return malformed("invalid character in character data");
                    }
                    self.advance(other.len_utf8());
                    into.push(other);
                }
            }
        }
        Ok(())
    }

    fn cdata(&mut self, into: &mut String) -> Result<(), String> {
        self.advance("<![CDATA[".len());
        let rest = self.rest();
        let end = rest.find("]]>").ok_or("malformed XML: unterminated CDATA section")?;
        let body = rest.get(..end).unwrap_or_default();
        let mut previous_cr = false;
        for c in body.chars() {
            if !is_xml_char(c) {
                return malformed("invalid character in a CDATA section");
            }
            match c {
                '\r' => {
                    into.push('\n');
                    previous_cr = true;
                    continue;
                }
                '\n' if previous_cr => {}
                other => into.push(other),
            }
            previous_cr = false;
        }
        self.advance(end + 3);
        Ok(())
    }

    /// Append character data to the innermost open element (`text` before its
    /// first child, otherwise the previous child's `tail`).
    fn emit(stack: &mut [Element], data: &str) {
        if data.is_empty() {
            return;
        }
        if let Some(open) = stack.last_mut() {
            match open.children.last_mut() {
                Some(previous) => previous.tail.push_str(data),
                None => open.text.push_str(data),
            }
        }
    }

    fn element_tree(&mut self) -> Result<Element, String> {
        let mut stack: Vec<Element> = Vec::new();
        let mut elements = 0_usize;
        let mut root: Option<Element> = None;
        loop {
            if stack.is_empty() && root.is_some() {
                return root.ok_or_else(|| "internal error".to_owned());
            }
            if self.starts_with("</") {
                self.advance(2);
                let name = self.name()?;
                self.skip_spaces();
                if !self.starts_with(">") {
                    return malformed("junk in an end tag");
                }
                self.advance(1);
                let Some(done) = stack.pop() else {
                    return malformed("unexpected end tag");
                };
                if done.tag != name {
                    return malformed("mismatched tag");
                }
                match stack.last_mut() {
                    Some(parent) => parent.children.push(done),
                    None => root = Some(done),
                }
            } else if self.starts_with("<!--") {
                self.comment()?;
            } else if self.starts_with("<![CDATA[") {
                if stack.is_empty() {
                    return malformed("CDATA outside the root element");
                }
                let mut data = String::new();
                self.cdata(&mut data)?;
                Self::emit(&mut stack, &data);
            } else if self.starts_with("<?") {
                self.processing_instruction()?;
            } else if self.starts_with("<!") {
                return malformed("unexpected declaration inside the document");
            } else if self.starts_with("<") {
                self.advance(1);
                if stack.len() >= self.limits.max_depth {
                    return Err(format!("XML nesting deeper than {}; refusing to parse", self.limits.max_depth));
                }
                elements += 1;
                if elements > self.limits.max_elements {
                    return Err(format!("XML has more than {} elements; refusing to parse", self.limits.max_elements));
                }
                let tag = self.name()?.to_owned();
                let mut attrs: Vec<(String, String)> = Vec::new();
                loop {
                    let spaced = self.skip_spaces();
                    if self.starts_with("/>") {
                        self.advance(2);
                        let element = Element { tag, attrs, ..Element::default() };
                        match stack.last_mut() {
                            Some(parent) => parent.children.push(element),
                            None => root = Some(element),
                        }
                        break;
                    }
                    if self.starts_with(">") {
                        self.advance(1);
                        stack.push(Element { tag, attrs, ..Element::default() });
                        break;
                    }
                    if !spaced {
                        return malformed("expected whitespace before an attribute");
                    }
                    let name = self.name()?.to_owned();
                    self.skip_spaces();
                    if !self.starts_with("=") {
                        return malformed("expected '=' after an attribute name");
                    }
                    self.advance(1);
                    self.skip_spaces();
                    let value = self.attribute_value()?;
                    if attrs.iter().any(|(existing, _)| *existing == name) {
                        return malformed("duplicate attribute");
                    }
                    attrs.push((name, value));
                }
            } else if self.peek().is_none() {
                return malformed("unclosed element at the end of the document");
            } else {
                if stack.is_empty() {
                    return malformed("text outside the root element");
                }
                let mut data = String::new();
                self.char_data(&mut data)?;
                Self::emit(&mut stack, &data);
            }
        }
    }
}

fn rest_after(text: &str, bytes: usize) -> Option<char> {
    text.get(bytes..).and_then(|rest| rest.chars().next())
}

/// Python `str.strip()`.
pub fn py_strip(text: &str) -> &str {
    text.trim_matches(is_python_space)
}
