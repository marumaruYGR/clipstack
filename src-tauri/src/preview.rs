use std::{
    fs::File,
    io::{BufReader, Cursor, Read},
    path::{Component, Path, Prefix},
};

use base64::{engine::general_purpose::STANDARD, Engine};
use dom_query::{Document, NodeRef};
use encoding_rs::{Encoding, WINDOWS_1252};
use image::{ImageFormat, ImageReader, Limits};
use quick_xml::{events::Event, Reader};
use serde::Serialize;
use windows_sys::Win32::{
    Storage::FileSystem::GetDriveTypeW,
    System::WindowsProgramming::{DRIVE_CDROM, DRIVE_FIXED, DRIVE_RAMDISK, DRIVE_REMOVABLE},
};
use zip::ZipArchive;

use crate::clipboard::{html_fragment, Content};

const MAX_TEXT: usize = 1024 * 1024;
const MAX_IMAGE: u64 = 64 * 1024 * 1024;
const MAX_ARCHIVE: u64 = 32 * 1024 * 1024;
const TEXT_TRUNCATED: &str = "\n\n…预览已截断";
pub const MANUAL_ID: &str = "clipstack:user-guide";

#[derive(Serialize)]
pub struct Preview {
    pub kind: &'static str,
    pub title: String,
    pub text: Option<String>,
    pub image: Option<String>,
    pub html: Option<String>,
    pub files: Vec<String>,
    pub index: usize,
}

impl Preview {
    fn new(kind: &'static str, title: impl Into<String>) -> Self {
        Self {
            kind,
            title: title.into(),
            text: None,
            image: None,
            html: None,
            files: Vec::new(),
            index: 0,
        }
    }

    fn text(title: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            ..Self::new("text", title)
        }
    }

    fn markup(title: impl Into<String>, html: String) -> Self {
        Self {
            html: Some(html),
            ..Self::new("html", title)
        }
    }

    fn picture(title: impl Into<String>, bytes: &[u8]) -> Self {
        Self {
            image: Some(format!("data:image/png;base64,{}", STANDARD.encode(bytes))),
            ..Self::new("image", title)
        }
    }

    fn unavailable(title: &str, message: &str) -> Self {
        Self {
            text: Some(message.into()),
            ..Self::new("file", title)
        }
    }
}

pub fn manual() -> Preview {
    Preview {
        text: Some(include_str!("../../ui/manual/guide.md").into()),
        ..Preview::new("markdown", "使用手册")
    }
}

pub fn preview(content: &Content, index: usize) -> Result<Preview, String> {
    let result = match content {
        Content::Text { text } => Preview::text("文本预览", text.as_str()),
        Content::Link { text } => Preview::text("链接预览", text.as_str()),
        Content::Image { png, .. } => Preview::picture("图片预览", png),
        Content::RichText {
            text, html, rtf, ..
        } => {
            let rich = html
                .as_deref()
                .and_then(html_fragment)
                .map(sanitize_html)
                .filter(|safe| !safe.text.trim().is_empty())
                .or_else(|| rtf.as_deref().and_then(parse_rtf));
            match rich {
                Some(safe) => Preview::markup("富文本预览", safe.html),
                None if !text.is_empty() => Preview::text("文本预览", text.as_str()),
                None => Preview::unavailable(
                    "富文本预览",
                    "这条记录的格式暂时无法预览，仍可重新选取并粘贴。",
                ),
            }
        }
        Content::Files { paths, .. } => {
            let path = paths
                .get(index)
                .ok_or_else(|| "找不到此文件记录。".to_string())?;
            let path = Path::new(path);
            let mut result = preview_file(path);
            result.files.clone_from(paths);
            result.index = index;
            result
        }
    };
    Ok(result)
}

pub fn searchable_text(content: &Content) -> String {
    match content {
        Content::Text { text } | Content::Link { text } => text.clone(),
        Content::Image { rich, .. } => rich
            .as_ref()
            .map(|rich| {
                searchable_rich(
                    &rich.text,
                    rich.html.as_deref(),
                    rich.rtf.as_deref(),
                    rich.url.as_deref(),
                )
            })
            .unwrap_or_default(),
        Content::Files { paths, .. } => paths
            .iter()
            .map(|path| file_name(Path::new(path)))
            .collect::<Vec<_>>()
            .join("\n"),
        Content::RichText {
            text,
            html,
            rtf,
            url,
        } => searchable_rich(text, html.as_deref(), rtf.as_deref(), url.as_deref()),
    }
}

fn searchable_rich(
    text: &str,
    html: Option<&[u8]>,
    rtf: Option<&[u8]>,
    url: Option<&str>,
) -> String {
    let mut output = if text.trim().is_empty() {
        html.and_then(html_fragment)
            .map(sanitize_html)
            .filter(|safe| !safe.text.trim().is_empty())
            .or_else(|| rtf.and_then(parse_rtf))
            .map(|safe| safe.text)
            .unwrap_or_default()
    } else {
        text.to_string()
    };
    if let Some(url) = url.filter(|url| !output.contains(url)) {
        output.push('\n');
        output.push_str(url);
    }
    output
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

fn extension(path: &Path) -> String {
    path.extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase()
}

fn is_local_path(path: &Path) -> bool {
    let Some(Component::Prefix(prefix)) = path.components().next() else {
        return false;
    };
    let drive = match prefix.kind() {
        Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => drive,
        _ => return false,
    };
    if !path.has_root() || path.components().any(|part| part == Component::ParentDir) {
        return false;
    }
    let root = [u16::from(drive), 58, 92, 0];
    matches!(
        unsafe { GetDriveTypeW(root.as_ptr()) },
        DRIVE_FIXED | DRIVE_REMOVABLE | DRIVE_CDROM | DRIVE_RAMDISK
    )
}

fn preview_file(path: &Path) -> Preview {
    let title = file_name(path);
    if !is_local_path(path) {
        return Preview::unavailable(&title, "此位置不能直接预览。你可以在资源管理器中查看文件。");
    }
    if !path.exists() {
        return Preview::unavailable(&title, "文件已移动、删除或无法访问。");
    }
    if !path.is_file() {
        return Preview::unavailable(&title, "请在资源管理器中查看此文件夹。");
    }

    let ext = extension(path);
    if raster_extension(&ext) {
        return match decode_image(path, 2048, 1536) {
            Ok(bytes) => Preview::picture(title, &bytes),
            Err(()) => {
                Preview::unavailable(&title, "无法读取此图片。原文件仍可在资源管理器中查看。")
            }
        };
    }
    let document = match ext.as_str() {
        "html" | "htm" => read_text_file(path)
            .ok()
            .map(|text| Preview::markup(&title, sanitize_html(&text).html)),
        "rtf" => read_bounded(path)
            .ok()
            .and_then(|(bytes, _)| parse_rtf(&bytes))
            .map(|safe| Preview::markup(&title, safe.html)),
        "pdf" => render_pdf(path).map(|(bytes, pages)| {
            Preview::picture(format!("{title}（第 1 页，共 {pages} 页）"), &bytes)
        }),
        "docx" | "docm" | "pptx" | "pptm" | "xlsx" | "xlsm" | "odt" | "ods" | "odp" => {
            archive_document_text(path, &ext).map(|text| Preview::text(&title, text))
        }
        "txt" | "md" | "markdown" | "csv" | "tsv" | "log" | "json" | "xml" | "yaml" | "yml"
        | "toml" | "ini" => read_text_file(path)
            .ok()
            .map(|text| Preview::text(&title, text)),
        _ => None,
    };
    document.unwrap_or_else(|| {
        Preview::unavailable(
            &title,
            "此文件无法在剪栈中预览。你可以在资源管理器中定位原文件。",
        )
    })
}

fn raster_extension(ext: &str) -> bool {
    matches!(
        ext,
        "png" | "apng" | "jpg" | "jpeg" | "jfif" | "bmp" | "gif" | "webp" | "tif" | "tiff" | "ico"
    )
}

pub fn is_image_file(path: &str) -> bool {
    raster_extension(&extension(Path::new(path)))
}

pub fn file_thumbnail(paths: &[String]) -> Option<String> {
    let png = paths
        .iter()
        .filter(|path| is_image_file(path))
        .take(12)
        .find_map(|path| {
            let path = Path::new(path);
            if is_local_path(path) {
                decode_image(path, 300, 180).ok()
            } else {
                None
            }
        })?;
    Some(format!("data:image/png;base64,{}", STANDARD.encode(png)))
}

fn decode_image(path: &Path, width: u32, height: u32) -> Result<Vec<u8>, ()> {
    let file = File::open(path).map_err(|_| ())?;
    let metadata = file.metadata().map_err(|_| ())?;
    if !metadata.is_file() || metadata.len() > MAX_IMAGE {
        return Err(());
    }
    let mut reader = ImageReader::new(BufReader::new(file))
        .with_guessed_format()
        .map_err(|_| ())?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(12160);
    limits.max_image_height = Some(12160);
    limits.max_alloc = Some(160 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().map_err(|_| ())?;
    let image = if image.width() > width || image.height() > height {
        image.thumbnail(width, height)
    } else {
        image
    };
    let mut png = Cursor::new(Vec::new());
    image.write_to(&mut png, ImageFormat::Png).map_err(|_| ())?;
    Ok(png.into_inner())
}

fn render_pdf(path: &Path) -> Option<(Vec<u8>, u32)> {
    use windows::{
        Data::Pdf::{PdfDocument, PdfPageRenderOptions},
        Storage::Streams::{DataReader, DataWriter, InMemoryRandomAccessStream},
        Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED},
    };

    let file = File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_ARCHIVE + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_ARCHIVE {
        return None;
    }

    // Tauri invokes this on a blocking worker so joining WinRT operations never blocks its UI thread.
    unsafe {
        RoInitialize(RO_INIT_MULTITHREADED).ok()?;
    }
    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe {
                RoUninitialize();
            }
        }
    }
    let _apartment = Apartment;

    let input = InMemoryRandomAccessStream::new().ok()?;
    let writer = DataWriter::CreateDataWriter(&input.GetOutputStreamAt(0).ok()?).ok()?;
    writer.WriteBytes(&bytes).ok()?;
    writer.StoreAsync().ok()?.join().ok()?;
    writer.DetachStream().ok()?;
    input.Seek(0).ok()?;
    let pdf = PdfDocument::LoadFromStreamAsync(&input).ok()?.join().ok()?;
    let pages = pdf.PageCount().ok()?;
    let page = pdf.GetPage(0).ok()?;
    let size = page.Size().ok()?;
    if !size.Width.is_finite()
        || !size.Height.is_finite()
        || size.Width <= 0.0
        || size.Height <= 0.0
    {
        return None;
    }
    let scale = 1600.0 / size.Width.max(size.Height);
    let options = PdfPageRenderOptions::new().ok()?;
    options
        .SetDestinationWidth((size.Width * scale).round().max(1.0) as u32)
        .ok()?;
    options
        .SetDestinationHeight((size.Height * scale).round().max(1.0) as u32)
        .ok()?;

    let output = InMemoryRandomAccessStream::new().ok()?;
    page.RenderWithOptionsToStreamAsync(&output, &options)
        .ok()?
        .join()
        .ok()?;
    let length = output.Size().ok()?.min(MAX_IMAGE + 1) as u32;
    if u64::from(length) > MAX_IMAGE {
        return None;
    }
    let reader = DataReader::CreateDataReader(&output.GetInputStreamAt(0).ok()?).ok()?;
    if reader.LoadAsync(length).ok()?.join().ok()? != length {
        return None;
    }
    let mut pixels = vec![0; length as usize];
    reader.ReadBytes(&mut pixels).ok()?;
    let image = image::load_from_memory(&pixels).ok()?;
    let mut png = Cursor::new(Vec::new());
    image.write_to(&mut png, ImageFormat::Png).ok()?;
    Some((png.into_inner(), pages))
}

fn read_bounded(path: &Path) -> std::io::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_TEXT as u64 + 1)
        .read_to_end(&mut bytes)?;
    let truncated = bytes.len() > MAX_TEXT;
    bytes.truncate(MAX_TEXT);
    Ok((bytes, truncated))
}

fn read_text_file(path: &Path) -> std::io::Result<String> {
    let (bytes, truncated) = read_bounded(path)?;
    let (encoding, bom) = Encoding::for_bom(&bytes).unwrap_or((encoding_rs::UTF_8, 0));
    let mut text = if bom == 0 && std::str::from_utf8(&bytes).is_err() {
        WINDOWS_1252
            .decode_without_bom_handling(&bytes)
            .0
            .into_owned()
    } else {
        encoding
            .decode_without_bom_handling(&bytes[bom..])
            .0
            .into_owned()
    };
    if text.contains('\0') {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    if truncated {
        text.push_str(TEXT_TRUNCATED);
    }
    Ok(text)
}

#[derive(Default)]
struct SafeMarkup {
    text: String,
    html: String,
}

fn append_escaped(html: &mut String, source: &str) {
    for ch in source.chars() {
        match ch {
            '&' => html.push_str("&amp;"),
            '<' => html.push_str("&lt;"),
            '>' => html.push_str("&gt;"),
            _ => html.push(ch),
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Emphasis {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
}

impl Emphasis {
    fn tags(self) -> impl DoubleEndedIterator<Item = &'static str> {
        [
            (self.bold, "strong"),
            (self.italic, "em"),
            (self.underline, "u"),
            (self.strike, "s"),
        ]
        .into_iter()
        .filter_map(|(enabled, tag)| enabled.then_some(tag))
    }
}

fn open_tag(html: &mut String, tag: &str) {
    html.push('<');
    html.push_str(tag);
    html.push('>');
}
fn close_tag(html: &mut String, tag: &str) {
    html.push_str("</");
    html.push_str(tag);
    html.push('>');
}

fn truncate_at_boundary(source: &str) -> &str {
    source
        .get(..source.floor_char_boundary(MAX_TEXT.min(source.len())))
        .unwrap_or_default()
}

fn sanitize_html(source: &str) -> SafeMarkup {
    let document = Document::from(truncate_at_boundary(source));
    let mut result = SafeMarkup::default();
    if let Some(body) = document.body() {
        for child in body.children_it(false) {
            visit_html(child, 0, &mut result);
        }
    }
    result
}

fn visit_html(node: NodeRef<'_>, depth: usize, safe: &mut SafeMarkup) {
    if depth >= 64 || safe.html.len() > MAX_TEXT * 2 {
        return;
    }
    let Some(name) = node.node_name() else {
        if node.is_text() {
            let text = node.text();
            safe.text.push_str(&text);
            append_escaped(&mut safe.html, &text);
        }
        return;
    };
    if matches!(
        name.as_ref(),
        "script"
            | "style"
            | "template"
            | "iframe"
            | "object"
            | "embed"
            | "svg"
            | "math"
            | "head"
            | "noscript"
    ) || node.attr("hidden").is_some()
    {
        return;
    }
    let mut emphasis = Emphasis::default();
    let mut alignment = None;
    if let Some(style) = node.attr("style") {
        let style = style.to_ascii_lowercase();
        for (key, value) in style.split(';').filter_map(|decl| decl.split_once(':')) {
            let value = value.trim();
            match (key.trim(), value) {
                ("display", "none") | ("visibility", "hidden") => return,
                ("font-weight", "bold" | "bolder" | "600" | "700" | "800" | "900") => {
                    emphasis.bold = true
                }
                ("font-style", "italic" | "oblique") => emphasis.italic = true,
                ("text-align", "left") => alignment = Some("left"),
                ("text-align", "center") => alignment = Some("center"),
                ("text-align", "right") => alignment = Some("right"),
                ("text-align", "justify") => alignment = Some("justify"),
                ("text-decoration" | "text-decoration-line", _) => {
                    emphasis.underline = value
                        .split_ascii_whitespace()
                        .any(|part| part == "underline");
                    emphasis.strike = value
                        .split_ascii_whitespace()
                        .any(|part| part == "line-through");
                }
                _ => {}
            }
        }
    }
    let tag = match name.as_ref() {
        tag @ ("section" | "div" | "p" | "br" | "hr" | "blockquote" | "pre" | "code" | "ul"
        | "ol" | "li" | "table" | "thead" | "tbody" | "tfoot" | "tr" | "th" | "td"
        | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "b" | "i" | "strong" | "em" | "u"
        | "s" | "del" | "strike" | "sup" | "sub" | "span") => Some(tag),
        _ => None,
    };
    let block = matches!(
        tag,
        Some(
            "section"
                | "div"
                | "p"
                | "br"
                | "hr"
                | "blockquote"
                | "pre"
                | "li"
                | "tr"
                | "th"
                | "td"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
        )
    );
    if block {
        safe.text.push(' ');
    }
    if let Some(tag) = tag {
        safe.html.push('<');
        safe.html.push_str(tag);
        if let Some(value) = alignment {
            safe.html.push_str(" style=\"text-align:");
            safe.html.push_str(value);
            safe.html.push('"');
        }
        if matches!(tag, "td" | "th") {
            for key in ["colspan", "rowspan"] {
                if let Some(n) = node
                    .attr(key)
                    .and_then(|value| value.parse::<u8>().ok())
                    .filter(|n| (1..=64).contains(n))
                {
                    safe.html.push(' ');
                    safe.html.push_str(key);
                    safe.html.push_str("=\"");
                    safe.html.push_str(&n.to_string());
                    safe.html.push('"');
                }
            }
        }
        safe.html.push('>');
    }
    if matches!(tag, Some("br" | "hr")) {
        return;
    }
    for tag in emphasis.tags() {
        open_tag(&mut safe.html, tag);
    }
    for child in node.children_it(false) {
        visit_html(child, depth + 1, safe);
    }
    for tag in emphasis.tags().rev() {
        close_tag(&mut safe.html, tag);
    }
    if let Some(tag) = tag {
        close_tag(&mut safe.html, tag);
    }
    if block {
        safe.text.push(' ');
    }
}

#[derive(Clone, Copy)]
struct RtfState {
    emphasis: Emphasis,
    encoding: &'static Encoding,
    unicode_fallback: usize,
    hidden: bool,
}

struct RtfOutput {
    markup: SafeMarkup,
    emphasis: Emphasis,
}

impl RtfOutput {
    fn format(&mut self, next: Emphasis) {
        if self.emphasis != next {
            for tag in self.emphasis.tags().rev() {
                close_tag(&mut self.markup.html, tag);
            }
            for tag in next.tags() {
                open_tag(&mut self.markup.html, tag);
            }
            self.emphasis = next;
        }
    }

    fn text(&mut self, text: &str, style: Emphasis) {
        if !text.is_empty() {
            self.format(style);
            self.markup.text.push_str(text);
            append_escaped(&mut self.markup.html, text);
        }
    }

    fn line(&mut self) {
        self.markup.html.push_str("<br>");
        self.markup.text.push('\n');
    }
}

fn rtf_encoding(page: i32) -> &'static Encoding {
    match page {
        65001 => encoding_rs::UTF_8,
        932 => encoding_rs::SHIFT_JIS,
        936 | 54936 => encoding_rs::GBK,
        949 => encoding_rs::EUC_KR,
        950 => encoding_rs::BIG5,
        874 => encoding_rs::WINDOWS_874,
        1250..=1258 => {
            Encoding::for_label(format!("windows-{page}").as_bytes()).unwrap_or(WINDOWS_1252)
        }
        _ => WINDOWS_1252,
    }
}

fn parse_rtf(raw: &[u8]) -> Option<SafeMarkup> {
    let raw = raw.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(raw);
    if !raw.starts_with(b"{\\rtf") {
        return None;
    }
    let bytes = &raw[..raw.len().min(MAX_TEXT)];
    let mut groups = Vec::new();
    let mut state = RtfState {
        emphasis: Emphasis::default(),
        encoding: WINDOWS_1252,
        unicode_fallback: 1,
        hidden: false,
    };
    let mut out = RtfOutput {
        markup: SafeMarkup {
            html: "<div>".into(),
            ..SafeMarkup::default()
        },
        emphasis: Emphasis::default(),
    };
    let mut skip = 0;
    let mut cursor = 0;
    let mut encoded = Vec::new();
    let mut unicode = Vec::new();
    let flush =
        |encoded: &mut Vec<u8>, unicode: &mut Vec<u16>, state: RtfState, out: &mut RtfOutput| {
            if !state.hidden {
                if !unicode.is_empty() {
                    out.text(&String::from_utf16_lossy(unicode), state.emphasis);
                }
                if !encoded.is_empty() {
                    out.text(
                        &state.encoding.decode_without_bom_handling(encoded).0,
                        state.emphasis,
                    );
                }
            }
            unicode.clear();
            encoded.clear();
        };
    while cursor < bytes.len() && out.markup.html.len() < MAX_TEXT * 2 {
        let byte = bytes[cursor];
        cursor += 1;
        match byte {
            b'{' | b'}' => {
                flush(&mut encoded, &mut unicode, state, &mut out);
                skip = 0;
                if byte == b'{' {
                    if groups.len() >= 128 {
                        break;
                    }
                    groups.push(state);
                } else if let Some(parent) = groups.pop() {
                    state = parent;
                }
                if groups.is_empty() {
                    break;
                }
            }
            b'\r' | b'\n' | 0 => {}
            b'\\' => {
                let Some(&token) = bytes.get(cursor) else {
                    break;
                };
                cursor += 1;
                if !token.is_ascii_alphabetic() {
                    let literal = match token {
                        b'\\' | b'{' | b'}' => Some(token),
                        b'\'' => {
                            let value = bytes
                                .get(cursor..cursor + 2)
                                .and_then(|hex| std::str::from_utf8(hex).ok())
                                .and_then(|hex| u8::from_str_radix(hex, 16).ok());
                            cursor = (cursor + 2).min(bytes.len());
                            value
                        }
                        _ => None,
                    };
                    if skip > 0 {
                        skip -= 1;
                        continue;
                    }
                    if let Some(value) = literal {
                        if !unicode.is_empty() {
                            flush(&mut encoded, &mut unicode, state, &mut out);
                        }
                        encoded.push(value);
                    } else {
                        flush(&mut encoded, &mut unicode, state, &mut out);
                        match token {
                            b'*' => state.hidden = true,
                            b'~' if !state.hidden => out.text("\u{00a0}", state.emphasis),
                            b'_' if !state.hidden => out.text("\u{2011}", state.emphasis),
                            _ => {}
                        }
                    }
                    continue;
                }
                let word_start = cursor - 1;
                while bytes.get(cursor).is_some_and(u8::is_ascii_alphabetic) {
                    cursor += 1;
                }
                let word = &bytes[word_start..cursor];
                let number_start = cursor;
                if bytes.get(cursor) == Some(&b'-') {
                    cursor += 1;
                }
                while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
                    cursor += 1;
                }
                let number = std::str::from_utf8(&bytes[number_start..cursor])
                    .ok()
                    .and_then(|value| value.parse::<i32>().ok());
                if bytes.get(cursor) == Some(&b' ') {
                    cursor += 1;
                }

                if word == b"bin" {
                    cursor = cursor
                        .saturating_add(number.unwrap_or(0).max(0) as usize)
                        .min(bytes.len());
                    continue;
                }
                if skip > 0 && word != b"u" {
                    skip -= 1;
                    continue;
                }
                if word == b"u" {
                    if !encoded.is_empty() {
                        flush(&mut encoded, &mut unicode, state, &mut out);
                    }
                    if let Some(value) = number {
                        unicode.push(value as u16);
                    }
                    skip = state.unicode_fallback;
                    continue;
                }
                flush(&mut encoded, &mut unicode, state, &mut out);
                match word {
                    b"fonttbl" | b"colortbl" | b"stylesheet" | b"info" | b"pict" | b"object"
                    | b"filetbl" | b"listtable" | b"listoverridetable" | b"header" | b"headerl"
                    | b"headerr" | b"footer" | b"footerl" | b"footerr" | b"fldinst" => {
                        state.hidden = true
                    }
                    b"ansicpg" => state.encoding = rtf_encoding(number.unwrap_or(1252)),
                    b"uc" => state.unicode_fallback = number.unwrap_or(1).clamp(0, 16) as usize,
                    b"b" => state.emphasis.bold = number != Some(0),
                    b"i" => state.emphasis.italic = number != Some(0),
                    b"ul" => state.emphasis.underline = number != Some(0),
                    b"ulnone" => state.emphasis.underline = false,
                    b"strike" => state.emphasis.strike = number != Some(0),
                    b"plain" => state.emphasis = Emphasis::default(),
                    b"par" | b"line" | b"row" if !state.hidden => out.line(),
                    b"tab" | b"cell" if !state.hidden => out.text("\t", state.emphasis),
                    b"emdash" if !state.hidden => out.text("—", state.emphasis),
                    b"endash" if !state.hidden => out.text("–", state.emphasis),
                    b"bullet" if !state.hidden => out.text("•", state.emphasis),
                    b"lquote" if !state.hidden => out.text("‘", state.emphasis),
                    b"rquote" if !state.hidden => out.text("’", state.emphasis),
                    b"ldblquote" if !state.hidden => out.text("“", state.emphasis),
                    b"rdblquote" if !state.hidden => out.text("”", state.emphasis),
                    _ => {}
                }
            }
            _ if skip != 0 => skip -= 1,
            _ => {
                if !unicode.is_empty() {
                    flush(&mut encoded, &mut unicode, state, &mut out);
                }
                encoded.push(byte);
            }
        }
    }
    flush(&mut encoded, &mut unicode, state, &mut out);
    if raw.len() > MAX_TEXT {
        out.text(TEXT_TRUNCATED, Emphasis::default());
    }
    out.format(Emphasis::default());
    out.markup.html.push_str("</div>");
    (!out.markup.text.trim().is_empty()).then_some(out.markup)
}

fn archive_document_text(path: &Path, extension: &str) -> Option<String> {
    let file = File::open(path).ok()?;
    if file.metadata().ok()?.len() > MAX_ARCHIVE {
        return None;
    }
    let mut archive = ZipArchive::new(BufReader::new(file)).ok()?;
    let vocabulary = match extension {
        "docx" | "docm" => XmlVocabulary::Word,
        "pptx" | "pptm" => XmlVocabulary::Slides,
        "xlsx" | "xlsm" => XmlVocabulary::Sheets,
        _ => XmlVocabulary::Odf,
    };
    let mut entries = match vocabulary {
        XmlVocabulary::Word => vec!["word/document.xml".to_string()],
        XmlVocabulary::Odf => vec!["content.xml".to_string()],
        XmlVocabulary::Slides | XmlVocabulary::Sheets => {
            let prefix = if matches!(vocabulary, XmlVocabulary::Slides) {
                "ppt/slides/slide"
            } else {
                "xl/worksheets/sheet"
            };
            let mut entries: Vec<_> = archive
                .file_names()
                .filter_map(|name| {
                    let number = name
                        .strip_prefix(prefix)?
                        .strip_suffix(".xml")?
                        .parse::<usize>()
                        .ok()?;
                    Some((number, name.to_string()))
                })
                .collect();
            entries.sort_unstable_by_key(|entry| entry.0);
            entries.into_iter().take(8).map(|entry| entry.1).collect()
        }
    };
    if matches!(vocabulary, XmlVocabulary::Sheets)
        && archive.index_for_name("xl/sharedStrings.xml").is_some()
    {
        // Shared strings hold cell text. Reading that fixed part avoids formula or external link resolution.
        entries.insert(0, "xl/sharedStrings.xml".to_string());
    }
    let mut output = String::new();
    for name in entries {
        let mut bytes = Vec::new();
        let entry = archive.by_name(&name).ok()?;
        if entry.size() > MAX_TEXT as u64 {
            continue;
        }
        entry
            .take(MAX_TEXT as u64 + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() > MAX_TEXT {
            continue;
        }
        let text = xml_document_text(&bytes, vocabulary).ok()?;
        if output.len() + text.len() > MAX_TEXT {
            output.push_str(TEXT_TRUNCATED);
            break;
        }
        if !output.is_empty() && !text.trim().is_empty() {
            output.push_str("\n\n");
        }
        output.push_str(text.trim());
    }
    (!output.trim().is_empty()).then_some(output)
}

#[derive(Clone, Copy)]
enum XmlVocabulary {
    Word,
    Slides,
    Sheets,
    Odf,
}

fn xml_document_text(bytes: &[u8], vocabulary: XmlVocabulary) -> Result<String, quick_xml::Error> {
    let mut reader = Reader::from_reader(bytes);
    let mut output = String::new();
    let mut included = 0usize;
    let is_text = |name: &str| match vocabulary {
        XmlVocabulary::Word => name == "w:t",
        XmlVocabulary::Slides => name == "a:t",
        XmlVocabulary::Sheets => matches!(name, "t" | "v"),
        XmlVocabulary::Odf => matches!(name, "text:p" | "text:h"),
    };
    loop {
        match reader.read_event()? {
            Event::Start(start) if is_text(start.name().as_ref()) => included += 1,
            Event::End(end) => {
                let name = end.name();
                if is_text(name.as_ref()) {
                    included = included.saturating_sub(1);
                }
                match name.as_ref() {
                    "w:p" | "a:p" | "text:p" | "text:h" | "row" | "si" => output.push('\n'),
                    "w:tc" | "a:tc" | "c" => output.push('\t'),
                    _ => {}
                }
            }
            Event::Empty(empty) => match empty.name().as_ref() {
                "w:br" | "a:br" | "text:line-break" => output.push('\n'),
                "w:tab" | "text:tab" => output.push('\t'),
                "text:s" => output.push(' '),
                _ => {}
            },
            Event::Text(text) if included > 0 => output.push_str(text.as_ref()),
            Event::CData(text) if included > 0 => output.push_str(text.as_ref()),
            Event::GeneralRef(reference) if included > 0 => {
                if let Some(ch) = reference.resolve_char_ref()? {
                    output.push(ch);
                } else if let Some(value) =
                    quick_xml::escape::resolve_xml_entity(reference.as_ref())
                {
                    output.push_str(value);
                }
            }
            Event::DocType(_) => return Ok(String::new()),
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(output)
}
