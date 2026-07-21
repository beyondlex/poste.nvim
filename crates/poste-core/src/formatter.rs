use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Region {
    Separator(String),
    Comment(String),
    VarDef {
        name: String,
        value: String,
        raw: String,
        style: VarStyle,
    },
    RequestLine {
        method: String,
        url: String,
        version: Option<String>,
        raw: String,
    },
    Header {
        key: String,
        value: String,
        raw: String,
    },
    BlankLine,
    Body {
        content: String,
        content_type: Option<String>,
    },
    PreScript {
        code: String,
        style: ScriptStyle,
    },
    PostScript {
        code: String,
        style: ScriptStyle,
    },
    ExternalScript {
        path: String,
        script_type: ScriptType,
    },
    /// `< path` — file content include (JSON) or upload (form), resolved at runtime
    FileUpload(String),
    Prompt(String),
    Import {
        path: String,
        alias: Option<String>,
        raw: String,
    },
    Run {
        target: String,
        raw: String,
    },
    Raw(String),
}

impl Region {
    pub fn raw_text(&self) -> String {
        match self {
            Region::Separator(s) => s.clone(),
            Region::Comment(s) => format!("#{}\n", s),
            Region::VarDef { raw, .. } => raw.clone(),
            Region::RequestLine { raw, .. } => raw.clone(),
            Region::Header { raw, .. } => raw.clone(),
            Region::BlankLine => String::new(),
            Region::Body { content, .. } => content.clone(),
            Region::PreScript { code: _, style } => match style {
                ScriptStyle::Inline(s) => s.clone(),
                ScriptStyle::Multiline(lines) => {
                    let mut s = String::from("< {%\n");
                    let indented = Formatter::reindent_code(lines);
                    for l in &indented {
                        s.push_str(l);
                        s.push('\n');
                    }
                    s.push_str("%}");
                    s
                }
            },
            Region::PostScript { code: _, style } => match style {
                ScriptStyle::Inline(s) => s.clone(),
                ScriptStyle::Multiline(lines) => {
                    let mut s = String::from("> {%\n");
                    let indented = Formatter::reindent_code(lines);
                    for l in &indented {
                        s.push_str(l);
                        s.push('\n');
                    }
                    s.push_str("%}");
                    s
                }
            },
            Region::ExternalScript { path, script_type } => {
                let prefix = match script_type {
                    ScriptType::Pre => "< ",
                    ScriptType::Post => "> ",
                };
                format!("{}{}", prefix, path)
            }
            Region::FileUpload(s) => format!("< {}", s),
            Region::Prompt(s) => format!("<<{}", s),
            Region::Import { raw, .. } => raw.clone(),
            Region::Run { raw, .. } => raw.clone(),
            Region::Raw(s) => s.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum VarStyle {
    Simple,
    Multiline { terminator: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ScriptStyle {
    Inline(String),
    Multiline(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ScriptType {
    Pre,
    Post,
}

impl fmt::Display for Region {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.raw_text())
    }
}

pub struct Tokenizer;

impl Tokenizer {
    pub fn tokenize(content: &str) -> Vec<Region> {
        let mut regions = Vec::new();
        let lines: Vec<&str> = content.lines().collect();
        let mut i = 0;
        let mut in_body = false;
        let mut found_method_line = false;

        while i < lines.len() {
            let line = lines[i];
            let trimmed = line.trim();

            if let Some(region) = Self::try_parse_multiline_script(line, &lines, &mut i) {
                regions.push(region);
                continue;
            }

            if let Some(region) = Self::try_parse_multiline_var(line, &lines, &mut i) {
                regions.push(region);
                continue;
            }

            if trimmed.is_empty() {
                if found_method_line && !in_body {
                    in_body = true;
                }
                regions.push(Region::BlankLine);
                i += 1;
                continue;
            }

            if let Some(region) = Self::try_parse_import(trimmed, line) {
                regions.push(region);
                i += 1;
                continue;
            }

            if let Some(region) = Self::try_parse_run(trimmed, line) {
                regions.push(region);
                i += 1;
                continue;
            }

            if let Some(region) = Self::try_parse_separator(trimmed, line) {
                regions.push(region);
                i += 1;
                found_method_line = false;
                in_body = false;
                continue;
            }

            if let Some(region) = Self::try_parse_inline_script(trimmed, line) {
                regions.push(region);
                i += 1;
                continue;
            }

            if let Some(region) = Self::try_parse_external_script(trimmed, line) {
                regions.push(region);
                i += 1;
                continue;
            }

            if let Some(region) = Self::try_parse_file_upload(trimmed, line) {
                regions.push(region);
                i += 1;
                continue;
            }

            if let Some(region) = Self::try_parse_prompt(trimmed, line) {
                regions.push(region);
                i += 1;
                continue;
            }

            if let Some(region) = Self::try_parse_comment(trimmed, line) {
                regions.push(region);
                i += 1;
                continue;
            }

            if let Some(region) = Self::try_parse_var(trimmed, line) {
                regions.push(region);
                i += 1;
                continue;
            }

            if let Some(region) =
                Self::try_parse_request_line(trimmed, line, in_body, found_method_line)
            {
                regions.push(region);
                found_method_line = true;
                i += 1;
                continue;
            }

            if let Some(region) = Self::try_parse_header(trimmed, line, in_body) {
                regions.push(region);
                i += 1;
                continue;
            }

            regions.push(Region::Raw(line.to_string()));
            i += 1;
        }

        regions
    }

    fn try_parse_multiline_script(line: &str, lines: &[&str], i: &mut usize) -> Option<Region> {
        let trimmed = line.trim();
        let is_pre =
            (trimmed.starts_with("< {%") || trimmed.starts_with("<{%")) && !trimmed.contains("%}");
        let is_post =
            (trimmed.starts_with("> {%") || trimmed.starts_with(">{%")) && !trimmed.contains("%}");

        if !is_pre && !is_post {
            return None;
        }

        let mut code_lines = Vec::new();
        *i += 1;
        while *i < lines.len() {
            let l = lines[*i];
            if l.trim() == "%}" {
                *i += 1;
                break;
            }
            code_lines.push(l.to_string());
            *i += 1;
        }

        Some(if is_pre {
            Region::PreScript {
                code: code_lines.join("\n"),
                style: ScriptStyle::Multiline(code_lines),
            }
        } else {
            Region::PostScript {
                code: code_lines.join("\n"),
                style: ScriptStyle::Multiline(code_lines),
            }
        })
    }

    fn try_parse_multiline_var(line: &str, lines: &[&str], i: &mut usize) -> Option<Region> {
        let name = Self::parse_multiline_var_name(line)?;
        let mut value_lines = Vec::new();
        *i += 1;
        while *i < lines.len() {
            let l = lines[*i];
            if l.trim() == "<<<" {
                *i += 1;
                break;
            }
            value_lines.push(l.to_string());
            *i += 1;
        }
        let raw_value = value_lines.join("\n");
        Some(Region::VarDef {
            name: name.clone(),
            value: raw_value.clone(),
            raw: format!("@{} =>>>\n{}\n<<<", name, raw_value),
            style: VarStyle::Multiline {
                terminator: "<<<".to_string(),
            },
        })
    }

    fn parse_multiline_var_name(line: &str) -> Option<String> {
        let trimmed = line.trim();
        if !trimmed.starts_with('@') {
            return None;
        }
        let content = &trimmed[1..];
        if let Some((name, marker)) = content.split_once('=') {
            if marker.trim() == ">>>" && is_valid_var_name(name.trim()) {
                return Some(name.trim().to_string());
            }
        }
        if let Some((name, marker)) = content.split_once(|c: char| c.is_whitespace()) {
            if marker.trim() == ">>>" && is_valid_var_name(name.trim()) {
                return Some(name.trim().to_string());
            }
        }
        None
    }

    fn try_parse_import(trimmed: &str, raw_line: &str) -> Option<Region> {
        let rest = trimmed.strip_prefix("import ")?;
        let (path, alias) = if let Some(idx) = rest.find(" as ") {
            (
                rest[..idx].trim().to_string(),
                Some(rest[idx + 4..].trim().to_string()),
            )
        } else {
            (rest.trim().to_string(), None)
        };
        Some(Region::Import {
            path,
            alias,
            raw: raw_line.to_string(),
        })
    }

    fn try_parse_run(trimmed: &str, raw_line: &str) -> Option<Region> {
        let target = trimmed.strip_prefix("run ")?.trim().to_string();
        Some(Region::Run {
            target,
            raw: raw_line.to_string(),
        })
    }

    fn try_parse_separator(trimmed: &str, raw_line: &str) -> Option<Region> {
        trimmed
            .starts_with("###")
            .then(|| Region::Separator(raw_line.to_string()))
    }

    fn try_parse_inline_script(trimmed: &str, raw_line: &str) -> Option<Region> {
        let (is_pre, is_post) = (
            trimmed.starts_with("< {%") || trimmed.starts_with("<{%"),
            trimmed.starts_with("> {%") || trimmed.starts_with(">{%"),
        );
        if !(is_pre || is_post) || !trimmed.contains("%}") {
            return None;
        }
        let code_start = trimmed.find("{%").map(|p| p + 2).unwrap_or(2);
        let code_end = trimmed.rfind("%}").unwrap_or(trimmed.len());
        let code = trimmed[code_start..code_end].trim().to_string();
        Some(if is_pre {
            Region::PreScript {
                code,
                style: ScriptStyle::Inline(raw_line.to_string()),
            }
        } else {
            Region::PostScript {
                code,
                style: ScriptStyle::Inline(raw_line.to_string()),
            }
        })
    }

    fn try_parse_external_script(trimmed: &str, _raw_line: &str) -> Option<Region> {
        let (prefix, script_type) = if trimmed.starts_with("< ") && trimmed.ends_with(".lua") {
            ("< ", ScriptType::Pre)
        } else if trimmed.starts_with("> ") && trimmed.ends_with(".lua") {
            ("> ", ScriptType::Post)
        } else {
            return None;
        };
        let path = trimmed.strip_prefix(prefix)?.trim().to_string();
        if !path.contains("./") && !path.contains("../") {
            return None;
        }
        Some(Region::ExternalScript { path, script_type })
    }

    fn try_parse_file_upload(trimmed: &str, _raw_line: &str) -> Option<Region> {
        if !trimmed.starts_with("< ") || trimmed.contains("{%") {
            return None;
        }
        let path = trimmed.strip_prefix("< ")?.trim().to_string();
        Some(Region::FileUpload(path))
    }

    fn try_parse_prompt(trimmed: &str, _raw_line: &str) -> Option<Region> {
        let rest = trimmed
            .strip_prefix("<<")
            .or_else(|| trimmed.strip_prefix("# <<"))?
            .trim();
        Some(Region::Prompt(rest.to_string()))
    }

    fn try_parse_comment(trimmed: &str, _raw_line: &str) -> Option<Region> {
        trimmed.starts_with('#').then(|| {
            let text = trimmed.strip_prefix('#').unwrap_or("").to_string();
            Region::Comment(text)
        })
    }

    fn try_parse_var(trimmed: &str, raw_line: &str) -> Option<Region> {
        let (name, value) = Self::parse_var_line(trimmed)?;
        Some(Region::VarDef {
            name,
            value: value.clone(),
            raw: raw_line.to_string(),
            style: VarStyle::Simple,
        })
    }

    fn parse_var_line(line: &str) -> Option<(String, String)> {
        let trimmed = line.trim();
        if !trimmed.starts_with('@') {
            return None;
        }
        let content = &trimmed[1..];
        if let Some((name, value)) = content.split_once('=') {
            let name = name.trim().to_string();
            let value = value.trim().to_string();
            if !name.is_empty() && is_valid_var_name(&name) {
                return Some((name, value));
            }
        }
        if let Some((name, value)) = content.split_once(|c: char| c.is_whitespace()) {
            let name = name.trim().to_string();
            let value = value.trim().to_string();
            if !name.is_empty() && is_valid_var_name(&name) {
                return Some((name, value));
            }
        }
        None
    }

    fn try_parse_request_line(
        trimmed: &str,
        raw_line: &str,
        in_body: bool,
        found_method_line: bool,
    ) -> Option<Region> {
        if in_body || found_method_line {
            return None;
        }
        let method = trimmed.split_whitespace().next()?;
        if !Self::is_http_method(method) {
            return None;
        }
        let parts: Vec<&str> = trimmed.splitn(3, char::is_whitespace).collect();
        Some(Region::RequestLine {
            method: parts[0].to_string(),
            url: parts.get(1).map(|s| s.to_string()).unwrap_or_default(),
            version: parts.get(2).map(|s| s.to_string()),
            raw: raw_line.to_string(),
        })
    }

    fn try_parse_header(trimmed: &str, raw_line: &str, in_body: bool) -> Option<Region> {
        if in_body {
            return None;
        }
        let (key, value) = trimmed.split_once(':')?;
        let key_trimmed = key.trim();
        if key_trimmed.is_empty()
            || !is_valid_header_key(key_trimmed)
            || key_trimmed.starts_with('@')
        {
            return None;
        }
        Some(Region::Header {
            key: key_trimmed.to_string(),
            value: value.trim().to_string(),
            raw: raw_line.to_string(),
        })
    }

    fn is_http_method(s: &str) -> bool {
        matches!(
            s,
            "GET" | "POST" | "PUT" | "DELETE" | "PATCH" | "HEAD" | "OPTIONS" | "TRACE" | "CONNECT"
        )
    }
}

fn is_valid_var_name(name: &str) -> bool {
    name.chars().all(|c| c.is_alphanumeric() || c == '_')
}

fn is_valid_header_key(key: &str) -> bool {
    key.chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
}

pub struct Formatter;

struct ClassifiedBlock {
    preamble: Vec<String>,
    request_line: Option<(String, String, Option<String>)>,
    headers: Vec<(String, String)>,
    body: Vec<String>,
    body_separator: bool,
    post_scripts: Vec<String>,
    after_post: Vec<String>,
    trailing: Vec<String>,
}

impl Formatter {
    pub fn format(content: &str) -> String {
        let regions = Tokenizer::tokenize(content);
        Self::apply_rules(&regions)
    }

    fn apply_rules(regions: &[Region]) -> String {
        // Split regions into blocks at Separator boundaries
        let mut blocks: Vec<Vec<&Region>> = Vec::new();
        let mut current: Vec<&Region> = Vec::new();
        for r in regions {
            match r {
                Region::Separator(_) => {
                    if !current.is_empty() {
                        blocks.push(std::mem::take(&mut current));
                    }
                    current.push(r);
                }
                _ => {
                    current.push(r);
                }
            }
        }
        if !current.is_empty() {
            blocks.push(current);
        }

        let mut out = String::new();

        for (block_idx, block) in blocks.iter().enumerate() {
            // Check if this is a file-header block (no Separator)
            if !block.iter().any(|r| matches!(r, Region::Separator(_))) {
                Self::format_file_header(block, &mut out);
                continue;
            }

            // Block with a Separator at position 0
            if block_idx > 0 {
                out.push('\n');
            }

            // Emit ### line
            if let Some(Region::Separator(s)) = block.first() {
                let name = s.trim_start_matches("###").trim();
                if name.is_empty() {
                    out.push_str("###\n");
                } else {
                    out.push_str(&format!("### {}\n", name));
                }
            }

            Self::format_request_block(&block[1..], &mut out);
        }

        // Rule 8: trailing newline
        let trimmed = out.trim_end().to_string();
        if trimmed.is_empty() {
            String::new()
        } else {
            format!("{}\n", trimmed)
        }
    }

    fn format_file_header(regions: &[&Region], out: &mut String) {
        let mut import_lines: Vec<String> = Vec::new();
        let mut var_lines: Vec<String> = Vec::new();
        let mut has_imports = false;
        let mut has_vars = false;

        for r in regions {
            match r {
                Region::Import { .. } | Region::Run { .. } => {
                    import_lines.push(r.raw_text());
                    has_imports = true;
                }
                Region::VarDef {
                    name, value, style, ..
                } => {
                    match style {
                        VarStyle::Simple => var_lines.push(format!("@{} = {}", name, value)),
                        VarStyle::Multiline { .. } => {
                            var_lines.push(format!("@{} =>>>\n{}\n<<<", name, value));
                        }
                    }
                    has_vars = true;
                }
                Region::Comment(text) => {
                    var_lines.push(if text.is_empty() {
                        "#".into()
                    } else {
                        format!("#{}", text)
                    });
                    has_vars = true;
                }
                Region::Prompt(rest) => {
                    var_lines.push(format!("<<{}", rest));
                    has_vars = true;
                }
                _ => {}
            }
        }

        if !has_imports && !has_vars {
            return;
        }

        if has_imports {
            for line in &import_lines {
                out.push_str(line);
                out.push('\n');
            }
        }
        if has_imports && has_vars {
            out.push('\n');
        }
        if has_vars {
            for line in &var_lines {
                out.push_str(line);
                out.push('\n');
            }
        }
    }

    fn format_request_block(regions: &[&Region], out: &mut String) {
        let classified = Self::classify_request_block(regions);

        Self::emit_preamble(&classified.preamble, out);
        Self::emit_request_line(classified.request_line.as_ref(), out);
        Self::emit_headers(&classified.headers, out);
        Self::emit_body(
            &classified.body,
            classified.body_separator,
            &classified.post_scripts,
            out,
        );
        Self::emit_post_scripts(&classified.post_scripts, out);
        Self::emit_after_post(&classified.after_post, out);
        Self::emit_trailing(&classified.trailing, classified.request_line.is_some(), out);
    }

    fn classify_request_block(regions: &[&Region]) -> ClassifiedBlock {
        let mut preamble: Vec<String> = Vec::new();
        let mut request_line: Option<(String, String, Option<String>)> = None;
        let mut headers: Vec<(String, String)> = Vec::new();
        let mut body: Vec<String> = Vec::new();
        let mut post_scripts: Vec<String> = Vec::new();
        let mut after_post: Vec<String> = Vec::new();
        let mut trailing: Vec<String> = Vec::new();
        let mut found_req = false;
        let mut touched_body = false;
        let mut body_separator = false;
        let mut has_post = false;

        for r in regions {
            match r {
                Region::PreScript { .. } if !found_req => preamble.push(r.raw_text()),
                Region::VarDef {
                    name, value, style, ..
                } if !found_req => {
                    preamble.push(Self::format_var_def(name, value, style));
                }
                Region::RequestLine {
                    method,
                    url,
                    version,
                    ..
                } => {
                    request_line = Some((method.clone(), url.clone(), version.clone()));
                    found_req = true;
                }
                Region::Header { key, value, .. } if found_req && !touched_body => {
                    headers.push((Self::capitalize_header_key(key), value.clone()));
                }
                Region::PostScript { .. } => {
                    has_post = true;
                    post_scripts.push(r.raw_text());
                }
                Region::ExternalScript { script_type, .. } => {
                    let text = r.raw_text();
                    match script_type {
                        ScriptType::Pre if !found_req => preamble.push(text),
                        ScriptType::Post => {
                            has_post = true;
                            post_scripts.push(text);
                        }
                        _ => {}
                    }
                }
                Region::BlankLine => {
                    if has_post {
                        after_post.push(String::new());
                    } else if found_req && !touched_body {
                        body_separator = true;
                    } else if touched_body {
                        body.push(String::new());
                    }
                }
                Region::Comment(text) => {
                    let line = if text.is_empty() {
                        "#".into()
                    } else {
                        format!("#{}", text)
                    };
                    if has_post {
                        after_post.push(line);
                    } else if !found_req {
                        preamble.push(line);
                    } else {
                        touched_body = true;
                        body.push(line);
                    }
                }
                Region::Raw(s) => {
                    if has_post {
                        after_post.push(s.clone());
                    } else if found_req {
                        touched_body = true;
                        body.push(s.clone());
                    }
                }
                Region::FileUpload(_) => {
                    let text = r.raw_text();
                    if has_post {
                        after_post.push(text);
                    } else if found_req {
                        touched_body = true;
                        body.push(text);
                    }
                }
                Region::Import { .. } | Region::Run { .. } => {
                    trailing.push(r.raw_text());
                }
                Region::Prompt(_) => {
                    let text = r.raw_text();
                    if !found_req {
                        preamble.push(text);
                    } else if has_post {
                        after_post.push(text);
                    } else {
                        touched_body = true;
                        body.push(text);
                    }
                }
                _ => {}
            }
        }

        ClassifiedBlock {
            preamble,
            request_line,
            headers,
            body,
            body_separator,
            post_scripts,
            after_post,
            trailing,
        }
    }

    fn format_var_def(name: &str, value: &str, style: &VarStyle) -> String {
        match style {
            VarStyle::Simple => format!("@{} = {}", name, value),
            VarStyle::Multiline { .. } => format!("@{} =>>>\n{}\n<<<", name, value),
        }
    }

    fn emit_preamble(preamble: &[String], out: &mut String) {
        for s in preamble {
            out.push_str(s);
            out.push('\n');
        }
    }

    fn emit_request_line(
        request_line: Option<&(String, String, Option<String>)>,
        out: &mut String,
    ) {
        if let Some((method, url, version)) = request_line {
            out.push_str(method);
            out.push(' ');
            out.push_str(url);
            if let Some(v) = version {
                out.push(' ');
                out.push_str(v);
            }
            out.push('\n');
        }
    }

    fn emit_headers(headers: &[(String, String)], out: &mut String) {
        for (key, value) in headers {
            out.push_str(&format!("{}: {}\n", key, value));
        }
    }

    fn emit_body(body: &[String], body_separator: bool, post_scripts: &[String], out: &mut String) {
        let has_content = !body.is_empty() || (body_separator && !post_scripts.is_empty());
        if !has_content {
            return;
        }
        out.push('\n');

        if body.is_empty() {
            return;
        }

        let formatted = Self::try_format_json_body(body);
        Self::compress_blank_lines(&formatted, out);
    }

    /// Try to parse body as JSON and pretty-print it.
    /// Strips trailing comments/blank lines (not valid JSON) before parsing.
    /// Returns formatted lines on success with any trailing content reattached.
    fn try_format_json_body(body: &[String]) -> Vec<String> {
        let split = Self::find_json_body_end(body);
        let json_part = &body[..split];
        let trailing = &body[split..];
        let joined = json_part.join("\n");
        match serde_json::from_str::<serde_json::Value>(&joined) {
            Ok(value) => {
                let pretty = serde_json::to_string_pretty(&value).unwrap_or(joined);
                let mut result: Vec<String> = pretty.lines().map(|l| l.to_string()).collect();
                result.extend_from_slice(trailing);
                result
            }
            Err(_) => body.to_vec(),
        }
    }

    /// Find where the JSON body ends by skipping trailing comment/blank lines.
    fn find_json_body_end(body: &[String]) -> usize {
        let mut end = body.len();
        for line in body.iter().rev() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                end -= 1;
            } else {
                break;
            }
        }
        end
    }

    fn emit_post_scripts(post_scripts: &[String], out: &mut String) {
        for s in post_scripts {
            out.push_str(s);
            out.push('\n');
        }
    }

    fn emit_after_post(after_post: &[String], out: &mut String) {
        let stripped = Self::strip_trailing_blanks(after_post);
        Self::compress_blank_lines(&stripped, out);
    }

    fn emit_trailing(trailing: &[String], has_request: bool, out: &mut String) {
        if trailing.is_empty() {
            return;
        }
        if has_request {
            out.push('\n');
        }
        for s in trailing {
            out.push_str(s);
            out.push('\n');
        }
    }

    fn compress_blank_lines(lines: &[String], out: &mut String) {
        let mut prev_empty = false;
        for line in lines {
            let is_empty = line.is_empty();
            if is_empty && prev_empty {
                continue;
            }
            prev_empty = is_empty;
            out.push_str(line);
            out.push('\n');
        }
    }

    fn strip_trailing_blanks(lines: &[String]) -> Vec<String> {
        let mut last_non_empty = lines.len();
        for (idx, line) in lines.iter().enumerate().rev() {
            if line.is_empty() {
                last_non_empty = idx;
            } else {
                break;
            }
        }
        lines[..last_non_empty].to_vec()
    }

    fn capitalize_header_key(key: &str) -> String {
        key.split('-')
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    None => String::new(),
                    Some(c) => c.to_uppercase().to_string() + chars.as_str(),
                }
            })
            .collect::<Vec<_>>()
            .join("-")
    }

    /// Re-indent script code lines with 2-space nesting.
    /// Strips original indent, then applies structural indent based on Lua/JS keywords.
    fn reindent_code(lines: &[String]) -> Vec<String> {
        let stripped: Vec<&str> = lines.iter().map(|l| l.trim()).collect();
        let mut result: Vec<String> = Vec::with_capacity(lines.len());
        let mut indent: usize = 0;

        for line in &stripped {
            if line.is_empty() {
                result.push(String::new());
                continue;
            }

            let first_word = line.split_whitespace().next().unwrap_or("");
            let dedent = first_word.starts_with("end")
                || first_word == "else"
                || first_word == "elseif"
                || first_word == "until"
                || first_word.starts_with('}')
                || first_word.starts_with("})");

            if dedent && indent > 0 {
                indent -= 1;
            }

            result.push(format!("{:indent$}{}", "", line, indent = indent * 2));

            let trimmed = line.trim_end();
            let indent_next = trimmed.contains("function(")
                || trimmed.ends_with('{')
                || trimmed.ends_with("then")
                || trimmed.ends_with("do")
                || trimmed.ends_with("else")
                || trimmed.ends_with("elseif")
                || trimmed == "repeat";

            if indent_next {
                indent += 1;
            }
        }

        result
    }
}

#[cfg(test)]
mod tests;
