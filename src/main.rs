mod error;

use std::{
    cell::RefCell,
    env::var,
    fmt::Display,
    fs::{File, OpenOptions},
    io::{Read, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Command,
    sync::{mpsc::Receiver, Arc, Mutex},
};

use anyhow::Context;
use axum::{
    extract::{Extension, Query},
    http,
    http::status::StatusCode,
    response::{IntoResponse, Response},
};
use concisemark::{
    node::{Node, NodeTagName},
    Page,
};
use error::{Error, Result};
use nvim_agent::{NeovimClient, Value};
use once_cell::sync::Lazy;
use serde::Deserialize;
use tracing_subscriber::fmt::writer::MakeWriter;

const DEFAULT_PORT: u16 = 3008;
const DEFUALT_HOST: &str = "127.0.0.1";
const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
const PKG_NAME: &str = env!("CARGO_PKG_NAME");
static PREVIEW_FILE_PATH: Lazy<Arc<Mutex<Option<PathBuf>>>> =
    Lazy::new(|| Arc::new(Mutex::new(None)));
static PREVIEW_CSS_PATH: Lazy<Arc<Mutex<Option<PathBuf>>>> =
    Lazy::new(|| Arc::new(Mutex::new(None)));

#[derive(Deserialize)]
enum FileTag {
    #[serde(rename = "css")]
    CSS,
    #[serde(rename = "path")]
    Path,
}

#[derive(Deserialize)]
struct FileMeta {
    tag: FileTag,
    val: Option<String>,
}

pub fn code_highlight<S1: AsRef<str>, S2: AsRef<str>>(
    code: S1,
    typ: Option<S2>,
) -> Result<String> {
    let code = code.as_ref();
    let ss = syntect::parsing::SyntaxSet::load_defaults_newlines();
    // a quick and dirty syntax highlighting
    let mut syntax = if let Some(syntax) = ss.find_syntax_by_extension("bash") {
        syntax
    } else {
        ss.find_syntax_plain_text()
    };
    if let Some(typ) = typ {
        if let Some(s) = ss.find_syntax_by_extension(typ.as_ref()) {
            syntax = s;
        }
    };
    let ts = syntect::highlighting::ThemeSet::load_defaults();
    let theme = &ts.themes["base16-ocean.dark"];
    let code =
        syntect::html::highlighted_html_for_string(code, &ss, syntax, theme)
            .context("unable to highlighting your code")?;
    Ok(code)
}

fn server(config: PreviewerConfig) -> Result<()> {
    let config = Arc::new(config);
    let addr = format!("{DEFUALT_HOST}:{}", config.port)
        .parse::<SocketAddr>()
        .map_err(|e| anyerr!("failed to parse socket addr: {e:?}"))?;
    log::info!("web server start to listen at {}", addr.to_string());
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(5)
        .enable_all()
        .build()
        .map_err(|e| anyerr!("failed to build runtime: {e:?}"))?;
    rt.block_on(async {
        let app = axum::Router::new()
            .route("/", axum::routing::get(render))
            .route("/ping", axum::routing::get(ping))
            .route("/pdf", axum::routing::get(render_as_pdf))
            .route("/file", axum::routing::get(file))
            .fallback(fallback)
            .layer(Extension(config));
        axum::Server::bind(&addr)
            .serve(app.into_make_service())
            .await
            .unwrap();
    });
    Ok(())
}

async fn ping() -> impl IntoResponse {
    (http::status::StatusCode::OK, "").into_response()
}

async fn fallback(uri: http::Uri) -> impl IntoResponse {
    let (status, mime, content) = match uri.to_string().as_str() {
        "/favicon.ico" => (
            StatusCode::OK,
            "image/x-icon",
            include_bytes!("static/favicon.ico").to_vec(),
        ),
        _ => {
            log::warn!("unknown uri: {uri}");
            (
                StatusCode::NOT_FOUND,
                "text/plain",
                format!("No route for {uri}").as_bytes().to_vec(),
            )
        }
    };
    Response::builder()
        .status(status)
        .header(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_str(mime).unwrap(),
        )
        .body(axum::body::boxed(axum::body::Full::from(content)))
        .unwrap()
}

async fn file(
    Extension(config): Extension<Arc<PreviewerConfig>>,
    filemeta: Query<FileMeta>,
) -> impl IntoResponse {
    let filepath = match filemeta.tag {
        FileTag::CSS => {
            let path = PREVIEW_CSS_PATH.lock().unwrap();
            let p = path.clone();
            if let Some(pp) = p {
                pp
            } else {
                return (StatusCode::NOT_FOUND, "css file not found")
                    .into_response();
            }
        }
        FileTag::Path => {
            if let Some(val) = filemeta.val.as_deref() {
                Path::new(val).to_owned()
            } else {
                Path::new("").to_owned()
            }
        }
    };
    let mime = mime_guess::from_path(&filepath).first_or_text_plain();
    let mut mime = mime.as_ref();
    let mut content = vec![];
    if let Ok(mut f) = File::open(&filepath) {
        _ = f.read_to_end(&mut content);
    }
    if content.is_empty() {
        mime = "text/plain";
        content.extend_from_slice(
            format!("can not read file: {}", filepath.display()).as_bytes(),
        );
    }
    Response::builder()
        .status(StatusCode::OK)
        .header(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_str(mime).unwrap(),
        )
        .body(axum::body::boxed(axum::body::Full::from(content)))
        .unwrap()
}

#[derive(Deserialize)]
struct PDFOptions {
    is_source: Option<bool>,
}

async fn render_as_pdf(
    Extension(config): Extension<Arc<PreviewerConfig>>,
    options: Query<PDFOptions>,
) -> Result<axum::response::Response> {
    let enable_compile = options.is_source.is_none();

    let filepath = PREVIEW_FILE_PATH
        .lock()
        .map_err(|e| anyerr!("failed to lock: {e:?}"))?;
    let filepath = filepath.as_ref().ok_or(anyerr!("no previewed file"))?;
    let filepath = filepath
        .canonicalize()
        .map_err(|e| anyerr!("failed to canonicalize filepath: {e:?}"))?;
    let mut preview_file = File::open(&filepath).map_err(|e| {
        anyerr!(
            "failed to open file {} with error: {e:?}",
            filepath.display()
        )
    })?;
    let mut content = String::new();
    _ = preview_file.read_to_string(&mut content);

    let filedir = filepath
        .parent()
        .ok_or(anyerr!("preview file has no parent directory"))?;
    let workdir = tempfile::tempdir()
        .map_err(|e| anyerr!("failed to create temporary directory: {e:?}"))?;
    let page = Page::new(content);
    let hook = |node: &Node| -> Result<()> {
        let mut nodedata = node.data.borrow_mut();
        if nodedata.tag.name == NodeTagName::Image {
            let src = nodedata
                .tag
                .attrs
                .get("src")
                .ok_or(anyerr!("image source is empty"))?;
            let name = nodedata
                .tag
                .attrs
                .get("name")
                .unwrap_or(&"".to_owned())
                .to_owned();
            let mut imgpath = Path::new(&src).to_path_buf();
            if src.starts_with("https://") || src.starts_with("http://") {
                if !filedir.join(&name).exists() {
                    imgpath = concisemark::utils::download_image_fs(
                        src, filedir, &name,
                    )
                    .ok_or(anyerr!("failed to download media file {name}"))?;
                }
            } else {
                if filedir.join(src).exists() {
                    imgpath = filedir.join(src);
                }
            }

            if enable_compile {
                // Latex cannot embed svg image directly, we must convert svg to pdf.
                //
                // Note that if svg is generated from drawio, then you must disable `Word Wrap` and
                // `Formatted Text` or else your PDF will have an annoying message
                // `Text is not SVG - cannot display`, see [here](https://www.diagrams.net/doc/faq/svg-export-text-problems)
                // for detail.
                if let Some(imgext) = imgpath.extension() {
                    if imgext == "svg" {
                        let mut pdfpath = imgpath.clone();
                        pdfpath.set_extension("pdf");
                        let mut cmd = Command::new("rsvg-convert");
                        let output = cmd
                            .arg(format!("{}", imgpath.display()))
                            .arg("-o")
                            .arg(format!("{}", pdfpath.display()))
                            .arg("-f")
                            .arg("Pdf")
                            .output()
                            .map_err(|e| {
                                anyerr!("failed to run rsvg-convert: {e:?}")
                            })?;
                        if !output.status.success() {
                            let errmsg = String::from_utf8(output.stderr)
                                .unwrap_or("failed to run".to_owned());
                            log::error!(
                                "rsvg-convert exit with error: {errmsg}"
                            );
                        }
                        imgpath = pdfpath
                    }
                }
            }

            nodedata
                .tag
                .attrs
                .insert("src".to_owned(), format!("{}", imgpath.display()));
        }
        Ok(())
    };
    page.transform(hook);

    let latex = page.render_latex();
    let texfile = workdir.path().join("output.tex");
    let mut f = OpenOptions::new()
        .truncate(true)
        .write(true)
        .create(true)
        .open(&texfile)
        .map_err(|e| anyerr!("failed to open texfile to write: {e:?}"))?;
    f.write(latex.as_bytes())
        .map_err(|e| anyerr!("failed to write texfile: {e:?}"))?;

    if enable_compile {
        let mut cmd = Command::new("xelatex");
        cmd.current_dir(&workdir);
        cmd.arg(&texfile);
        let output = cmd
            .output()
            .map_err(|e| anyerr!("failed to compile latex file: {e:?}"))?;
        if !output.status.success() {
            let errmsg = String::from_utf8(output.stdout)
                .unwrap_or("failed to compile".to_owned());
            return Err(Error::Other(anyerr!(
                "xelatex exit with error: {errmsg}"
            )));
        }
        let pdffile = workdir.path().join("output.pdf");
        let mut f = File::open(pdffile)
            .map_err(|e| anyerr!("failed to open rendered file: {e:?}"))?;
        let mut pdfbuf = vec![];
        _ = f.read_to_end(&mut pdfbuf);
        log::info!("render latex is done: {}", workdir.path().display());
        Ok(Response::builder()
            .status(StatusCode::OK)
            .header(
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_str("application/pdf")
                    .map_err(|e| anyerr!("failed to parse pdf mime: {e:?}"))?,
            )
            .body(axum::body::boxed(axum::body::Full::from(pdfbuf)))
            .map_err(|e| {
                anyerr!("failed to create pdf response body: {e:?}")
            })?)
    } else {
        Ok(Response::builder()
            .status(StatusCode::OK)
            .header(
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_str("text/plain; charset=utf-8")
                    .map_err(|e| {
                        anyerr!("failed to parse text/plain mime: {e:?}")
                    })?,
            )
            .body(axum::body::boxed(axum::body::Full::from(latex)))
            .map_err(|e| {
                anyerr!("failed to create pdf source response body: {e:?}")
            })?)
    }
}

async fn render(
    Extension(config): Extension<Arc<PreviewerConfig>>,
) -> impl IntoResponse {
    let mut meta = None;
    let html = match PREVIEW_FILE_PATH.lock().unwrap().as_ref() {
        Some(path) => {
            log::info!("start to render file: {}", path.display());
            let filedir = if let Some(d) = path.parent() { d } else { path };
            if let Ok(mut f) = File::open(path) {
                let mut content = String::new();
                _ = f.read_to_string(&mut content);
                let page = Page::new(&content);
                meta = page.meta.clone();
                let hook = |node: &Node| -> Result<()> {
                    let mut nodedata = node.data.borrow_mut();
                    if nodedata.tag.name == NodeTagName::Image {
                        let src =
                            if let Some(src) = nodedata.tag.attrs.get("src") {
                                src.to_owned()
                            } else {
                                "".to_owned()
                            };
                        let local_filepath = filedir.join(src);
                        if local_filepath.exists() {
                            let src = format!(
                                "http://{DEFUALT_HOST}:{}/file?tag=path&val={}",
                                config.port,
                                local_filepath.display(),
                            );
                            nodedata.tag.attrs.insert("src".to_owned(), src);
                        }
                    }
                    Ok(())
                };
                page.transform(hook);
                let hook = |node: &Node| -> Option<String> {
                    let nodedata = node.data.borrow_mut();
                    if nodedata.tag.name == NodeTagName::Code {
                        let (s, e) = (nodedata.range.start, nodedata.range.end);
                        let code = content[s..e].to_owned();
                        let code = code.trim_matches(|c| c == '`');
                        if nodedata.tag.attrs.contains_key("inlined") {
                            return None;
                        }
                        let code = concisemark::utils::remove_indent(code);
                        if let Ok(code) = code_highlight(&code, None::<&str>) {
                            return Some(code);
                        }
                        return Some(code.to_owned());
                    }
                    None
                };
                page.render_with_hook(&hook)
            } else {
                format!("failed to open file: {}", path.display())
            }
        }
        None => "no file to render".to_owned(),
    };
    let (title, subtitle, date) = if let Some(meta) = meta {
        let title = meta.title;
        let subtitle = meta.subtitle.unwrap_or("".to_owned());
        let date = format!("{}", meta.date.format("%Y-%m-%d %H:%M:%S"));
        (title, subtitle, date)
    } else {
        ("".to_owned(), "".to_owned(), "".to_owned())
    };
    let html_template = format!(
        include_str!("../plugin/index.html"),
        title = title,
        script = include_str!("../plugin/nvim-previewer.js"),
        gap = if subtitle.is_empty() { "" } else { " - " },
        subtitle = subtitle,
        date = date,
        body = html,
    );

    let url = css_inline::Url::parse(&format!(
        "http://{DEFUALT_HOST}:{}",
        config.port
    ))
    .ok();
    let html_template = tokio::task::spawn_blocking(|| {
        let inliner = css_inline::CSSInliner::options()
            .base_url(url)
            .load_remote_stylesheets(true)
            .build();
        match inliner.inline(&html_template) {
            Ok(v) => v,
            Err(e) => {
                log::error!("failed to inline css style: {e:?}");
                html_template
            }
        }
    })
    .await
    .unwrap();

    Response::builder()
        .status(StatusCode::OK)
        .header(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_str("text/html").unwrap(),
        )
        .body(axum::body::boxed(axum::body::Full::from(html_template)))
        .unwrap()
}

#[derive(Debug, Clone)]
pub struct PreviewerConfig {
    pub browser: Option<String>,
    pub port: u16,
}

impl Default for PreviewerConfig {
    fn default() -> Self {
        PreviewerConfig {
            browser: None,
            port: DEFAULT_PORT,
        }
    }
}

impl Display for PreviewerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut msg = String::new();
        if let Some(browser) = self.browser.as_ref() {
            msg.push_str(&format!("\nbrowser: {browser}\n"));
        }
        msg.push_str(&format!("port: {}\n", self.port));
        f.write_str(&msg)
    }
}

impl PreviewerConfig {
    pub fn new<S1, S2>(browser: S1, port: S2) -> Self
    where
        S1: AsRef<str>,
        S2: AsRef<str>,
    {
        let (browser, port) = (browser.as_ref().trim(), port.as_ref().trim());
        let mut config = PreviewerConfig::default();
        if !browser.is_empty() {
            config.browser = Some(browser.to_owned());
        }
        if let Ok(v) = port.parse::<u16>() {
            if v > 1024 {
                config.port = v
            }
        }
        config
    }
}

struct Previewer {
    client: RefCell<NeovimClient>,
    config: PreviewerConfig,
    receiver: Receiver<(String, Vec<Value>)>,
    logdir: PathBuf,
    cachedir: PathBuf,
}

impl Previewer {
    pub fn new(mut client: NeovimClient) -> Self {
        let receiver = client.start();

        let cachedir =
            PathBuf::from(client.eval("stdpath('cache')")).join(PKG_NAME);
        let browser = client.eval("g:nvim_previewer_browser");
        let port = client.eval("g:nvim_previewer_port");
        Self {
            receiver,
            config: PreviewerConfig::new(browser, port),
            client: RefCell::new(client),
            logdir: cachedir.join("logs"),
            cachedir,
        }
    }

    pub fn recv(&self) -> &Receiver<(String, Vec<Value>)> {
        &self.receiver
    }

    pub fn eval<S: AsRef<str>>(&self, vimcmd: S) -> String {
        self.client.borrow_mut().eval(vimcmd.as_ref())
    }

    fn preview(&self) -> Result<()> {
        let url = format!("http://{DEFUALT_HOST}:{}", self.config.port);
        let r = if let Some(browser) = &self.config.browser {
            open::with(url, browser)
        } else {
            open::that(url)
        };
        if let Err(e) = r {
            self.client
                .borrow_mut()
                .print(format!("failed to start browser: {e:?}"));
            if cfg!(target_os = "linux") && var("DISPLAY").is_err() {
                self.client.borrow_mut().print("If you are using X11, please check if DISPLAY variable is defined");
            }
            if cfg!(target_os = "macos") {
                self.client
                    .borrow_mut()
                    .print("You can check if your brower is tagged with attribute com.apple.quarantine, remove it if there is one and reboot your system");
            }
        }

        Ok(())
    }

    pub fn print<S: AsRef<str>>(&self, msg: S) {
        self.client.borrow_mut().print(msg.as_ref());
    }
}

#[tokio::main]
async fn main() {
    let previewer = Previewer::new(nvim_agent::new_client());

    let file_appender = tracing_appender::rolling::daily(
        previewer.logdir.as_path(),
        PKG_VERSION,
    );
    let (non_blocking_appender, _guard) =
        tracing_appender::non_blocking(file_appender);
    tracing_subscriber::fmt()
        .with_line_number(true)
        .with_ansi(false)
        .with_writer(non_blocking_appender.make_writer())
        .init();

    let config = previewer.config.clone();
    std::thread::spawn(move || {
        if let Err(e) = server(config) {
            log::error!("start server failed: {e:?}");
        }
    });

    let pingurl =
        format!("http://{DEFUALT_HOST}:{}/ping", previewer.config.port);
    while reqwest::get(&pingurl).await.is_err() {}
    log::info!("server started with configuration: {}", previewer.config);

    for (event, params) in previewer.recv() {
        let file_path = if let Some(Some(p)) =
            params.get(0).map(|x| x.as_str().map(|x| x.to_owned()))
        {
            p
        } else {
            previewer.print("no file to be previewed");
            continue;
        };
        {
            let mut path = PREVIEW_FILE_PATH.lock().unwrap();
            *path = Some(Path::new(&file_path).to_owned())
        }
        log::info!("file path: {file_path}");

        let script_dir = if let Some(Some(p)) =
            params.get(1).map(|x| x.as_str().map(|x| x.to_owned()))
        {
            p
        } else {
            previewer.print("failed to find nvim-previewer plugin directory");
            continue;
        };
        log::info!("script directory: {script_dir}");

        let css_file_path = match event.as_str() {
            "preview_alt" => {
                Path::new(&script_dir).join("nvim-previewer-alt.css")
            }
            _ => Path::new(&script_dir).join("nvim-previewer-default.css"),
        };
        log::info!("css file path: {}", css_file_path.display());
        {
            let mut path = PREVIEW_CSS_PATH.lock().unwrap();
            *path = Some(css_file_path);
        }

        if let Err(e) = previewer.preview() {
            previewer.print(format!("{e:?}"));
        }
    }
}
