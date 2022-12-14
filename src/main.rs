mod error;

use error::Result;

use std::fmt::Display;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::cell::RefCell;
use std::net::SocketAddr;
use std::process::Command;

use axum::response::Response;
use axum::response::IntoResponse;
use axum::extract::Extension;
use axum::extract::Query;
use axum::http;
use axum::http::status::StatusCode;
use concisemark::Page;
use concisemark::node::Node;
use concisemark::node::NodeTagName;
use nvim_agent::Value;
use nvim_agent::NeovimClient;
use serde::Deserialize;
use tracing_subscriber::fmt::writer::MakeWriter;
use once_cell::sync::Lazy;

const DEFAULT_PORT: u16 = 3008;
const DEFUALT_HOST: &'static str = "127.0.0.1";
const PKG_VERSION: &'static str = env!("CARGO_PKG_VERSION");
const PKG_NAME: &'static str = env!("CARGO_PKG_NAME");
static PREVIEW_FILE_PATH: Lazy<Arc<Mutex<Option<String>>>> = Lazy::new(|| {
    Arc::new(Mutex::new(None))
});

#[derive(Deserialize)]
enum FileTag {
    #[serde(rename = "css")]
    CSS,
    #[serde(rename = "js")]
    JS,
    #[serde(rename = "path")]
    Path,
}

#[derive(Deserialize)]
struct FileMeta {
    tag: FileTag,
    val: Option<String>,
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
    let r = rt.block_on(async {
        let app = axum::Router::new()
            .route("/", axum::routing::get(render))
            .route("/pdf", axum::routing::get(render_as_pdf))
            .route("/file", axum::routing::get(file))
            .fallback(fallback)
            .layer(Extension(config));
        axum::Server::bind(&addr)
            .serve(app.into_make_service())
            .await.unwrap();
    });
    log::info!("web server exit with result: {r:?}");
    Ok(())
}

async fn fallback(uri: http::Uri) -> impl IntoResponse {
    let (status, mime, content) = match uri.to_string().as_str() {
        "/favicon.ico" => {
            (StatusCode::OK, "image/x-icon", include_bytes!("static/favicon.ico").to_vec())
        }
        _ => {
            log::warn!("unknown uri: {uri}");
            (StatusCode::NOT_FOUND, "text/plain", format!("No route for {uri}").as_bytes().to_vec())
        }
    };
    Response::builder().status(status)
        .header(http::header::CONTENT_TYPE, http::HeaderValue::from_str(mime).unwrap())
        .body(axum::body::boxed(axum::body::Full::from(content)))
        .unwrap()
}

async fn file(Extension(config): Extension<Arc<PreviewerConfig>>, filemeta: Query<FileMeta>) -> impl IntoResponse {
    let filepath = match filemeta.tag {
        FileTag::CSS => {
            config.css_file.as_path()
        }
        FileTag::JS => {
           config.js_file.as_path()
        }
        FileTag::Path => {
            if let Some(val) = filemeta.val.as_deref() {
                Path::new(val)
            } else {
                Path::new("")
            }
        }
    };
    let mime = mime_guess::from_path(filepath).first_or_text_plain();
    let mut mime = mime.as_ref();
    let mut content = vec![];
    if let Ok(mut f) = File::open(filepath) {
        _ = f.read_to_end(&mut content);
    }
    if content.len() == 0 {
        mime = "text/plain";
        content.extend_from_slice(format!("can not read file: {}", filepath.display()).as_bytes());
    }
    Response::builder().status(StatusCode::OK)
        .header(http::header::CONTENT_TYPE, http::HeaderValue::from_str(mime).unwrap())
        .body(axum::body::boxed(axum::body::Full::from(content)))
        .unwrap()
}

#[derive(Deserialize)]
struct PDFOptions {
    is_source: Option<bool>,
}

async fn render_as_pdf(Extension(config): Extension<Arc<PreviewerConfig>>, options: Query<PDFOptions>) -> Result<axum::response::Response> {
    let enable_compile = options.is_source.is_none();

    let filepath = PREVIEW_FILE_PATH.lock().map_err(|e| anyerr!("failed to lock: {e:?}"))?;
    let filepath = filepath.as_ref().ok_or(anyerr!("no previewed file"))?;
    let filepath = Path::new(filepath).canonicalize()
        .map_err(|e| anyerr!("failed to canonicalize filepath: {e:?}"))?;
    let mut preview_file = File::open(&filepath)
        .map_err(|e| anyerr!("failed to open file {} with error: {e:?}", filepath.display()))?;
    let mut content = String::new();
    _ = preview_file.read_to_string(&mut content);

    let filedir = filepath.parent().ok_or(anyerr!("preview file has no parent directory"))?;
    let workdir = tempfile::tempdir().map_err(|e| anyerr!("failed to create temporary directory: {e:?}"))?;
    let page = Page::new(content);
    let hook = |node: &Node| -> Result<()> {
        let mut nodedata = node.data.borrow_mut();
        if nodedata.tag.name == NodeTagName::Image {
            let src = nodedata.tag.attrs.get("src").ok_or(anyerr!("image source is empty"))?;
            let name = nodedata.tag.attrs.get("name").unwrap_or(&"".to_owned()).to_owned();
            let mut imgpath = Path::new(&src).to_path_buf();
            if src.starts_with("https://") || src.starts_with("http://") {
                if !filedir.join(&name).exists() {
                    imgpath = concisemark::utils::download_image_fs(&src, filedir, &name).ok_or(
                        anyerr!("failed to download media file {name}")
                    )?;
                }
            } else {
                if filedir.join(&src).exists() {
                    imgpath = filedir.join(&src);
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
                        if let Err(e) = cmd.arg(format!("{}", imgpath.display()))
                            .arg("-o")
                            .arg(format!("{}", pdfpath.display()))
                            .arg("-f")
                            .arg("Pdf")
                            .output() {
                            log::error!("failed to run rsvg-convert: {e:?}");
                        }
                        imgpath = pdfpath
                    }
                }
            }

            nodedata.tag.attrs.insert("src".to_owned(), format!("{}", imgpath.display()));
        }
        Ok(())
    };
    page.transform(hook);

    let latex = page.render_latex();
    let texfile = workdir.path().join("output.tex");
    let mut f = OpenOptions::new().truncate(true).write(true).create(true).open(&texfile).map_err(|e| anyerr!("failed to open texfile to write: {e:?}"))?;
    f.write(latex.as_bytes()).map_err(|e| anyerr!("failed to write texfile: {e:?}"))?;

    if enable_compile {
        let mut cmd = Command::new("xelatex");
        cmd.current_dir(&workdir);
        cmd.arg(&texfile);
        cmd.output().map_err(|e| anyerr!("failed to compile latex file: {e:?}"))?;
        let pdffile = workdir.path().join("output.pdf");
        let mut f = File::open(pdffile).map_err(|e| anyerr!("failed to open rendered file: {e:?}"))?;
        let mut pdfbuf = vec![];
        _ = f.read_to_end(&mut pdfbuf);
        log::info!("render latex is done: {}", workdir.path().display());
        Ok(Response::builder().status(StatusCode::OK)
            .header(http::header::CONTENT_TYPE, http::HeaderValue::from_str("application/pdf").map_err(|e| anyerr!("failed to parse pdf mime: {e:?}"))?)
            .body(axum::body::boxed(axum::body::Full::from(pdfbuf)))
            .map_err(|e| anyerr!("failed to create pdf response body: {e:?}"))?)
    } else {
         Ok(Response::builder().status(StatusCode::OK)
            .header(http::header::CONTENT_TYPE, http::HeaderValue::from_str("text/plain; charset=utf-8").map_err(|e| anyerr!("failed to parse text/plain mime: {e:?}"))?)
            .body(axum::body::boxed(axum::body::Full::from(latex)))
            .map_err(|e| anyerr!("failed to create pdf source response body: {e:?}"))?) 
    }
}

async fn render(Extension(config): Extension<Arc<PreviewerConfig>>) -> impl IntoResponse {
    let html = match PREVIEW_FILE_PATH.lock().unwrap().as_ref() {
        Some(path) => {
            log::info!("start to render file: {path}");
            let path = Path::new(path);
            let filedir = if let Some(d) = path.parent() {
                d
            } else {
                path
            };
            if let Ok(mut f) = File::open(path) {
                let mut content = String::new();
                _ = f.read_to_string(&mut content);
                let page = Page::new(content);
                let hook = |node: &Node| -> Result<()> {
                    let mut nodedata = node.data.borrow_mut();
                    if nodedata.tag.name == NodeTagName::Image {
                        let src = if let Some(src) = nodedata.tag.attrs.get("src") {
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
                page.render()
            } else {
                format!("failed to open file: {}", path.display())
            }
        }
        None => {
            "no file to render".to_owned()
        }
    };
    let html_template = format!(r#"
        <!DOCTYPE html>
        <html>
            <head>
                <title>{title}</title>

                <meta charset="utf-8">
                <meta name="format-detection" content="telephone=no">
                <meta name="msapplication-tap-highlight" content="no">
                <meta name="viewport" content="user-scalable=no, initial-scale=1, maximum-scale=1, minimum-scale=1">
                <link rel="stylesheet" type="text/css" href="/file?tag=css">
                <script src="/file?tag=js"></script>

                <link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/katex@0.16.3/dist/katex.min.css" integrity="sha384-Juol1FqnotbkyZUT5Z7gUPjQ9gzlwCENvUZTpQBAPxtusdwFLRy382PSDx5UUJ4/" crossorigin="anonymous">
                <script defer src="https://cdn.jsdelivr.net/npm/katex@0.16.3/dist/katex.min.js" integrity="sha384-97gW6UIJxnlKemYavrqDHSX3SiygeOwIZhwyOKRfSaf0JWKRVj9hLASHgFTzT+0O" crossorigin="anonymous"></script>
                <script defer src="https://cdn.jsdelivr.net/npm/katex@0.16.3/dist/contrib/auto-render.min.js" integrity="sha384-+VBxd3r6XgURycqtZ117nYw44OOcIax56Z4dCRWbxyPt0Koah1uHoK0o4+/RRE05" crossorigin="anonymous" onload="renderMathInElement(document.body);"> </script>
            </head>
            <body class="nvim-previewer">
                <div class="menu-bar">
                    <div class="right-menu">
                        <a href="/pdf">View as PDF</a>
                        <a href="/pdf?is_source=true">View Latex Source</a>
                    </div>
                </div>
                <article class="markdown-body">
                    {body}
                </article>
            </body>
        </html>
    "#,
        title = "Previewer",
        body = html,
    );
    Response::builder().status(StatusCode::OK)
        .header(http::header::CONTENT_TYPE, http::HeaderValue::from_str("text/html").unwrap())
        .body(axum::body::boxed(axum::body::Full::from(html_template)))
        .unwrap()
}

#[derive(Debug, Clone)]
pub struct PreviewerConfig {
    pub browser: Option<String>,
    pub port: u16,
    pub css_file: PathBuf,
    pub js_file: PathBuf,
}

impl Default for PreviewerConfig {
    fn default() -> Self {
        PreviewerConfig {
            browser: None,
            port: DEFAULT_PORT,
            css_file: PathBuf::new(),
            js_file: PathBuf::new(),
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
        msg.push_str(&format!("css_file: {}\n", self.css_file.display()));
        msg.push_str(&format!("js_file: {}\n", self.js_file.display()));
        f.write_str(&msg)
    }
}

impl PreviewerConfig {
    pub fn new<S1, S2, P1, P2>(browser: S1, port: S2, css_file: P1, js_file: P2) -> Self
    where
        S1: AsRef<str>, S2: AsRef<str>,
        P1: AsRef<Path>, P2: AsRef<Path>,
    {
        let (browser, port) = (browser.as_ref().trim(), port.as_ref().trim());
        let mut config = PreviewerConfig::default();
        if browser.len() > 0 {
            config.browser = Some(browser.to_owned());
        }
        if let Ok(v) = port.parse::<u16>() {
            if v > 1024 {
                config.port = v
            }
        }
        config.css_file = css_file.as_ref().to_path_buf();
        config.js_file = js_file.as_ref().to_path_buf();
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

        let cachedir = PathBuf::from(client.eval("stdpath('cache')")).join(PKG_NAME);
        let scriptdir = PathBuf::from(client.eval("g:nvim_previewer_script_dir"));
        let mut css_file = scriptdir.join(format!("{PKG_NAME}.css"));
        let mut js_file = scriptdir.join(format!("{PKG_NAME}.js"));

        let user_css_file = client.eval("g:nvim_previewer_css_file");
        let user_js_file = client.eval("g:nvim_previewer_js_file");
        if !user_css_file.is_empty() {
            let user_css_file = PathBuf::from(user_css_file);
            if user_css_file.exists() {
                css_file = user_css_file;
            } else {
                log::warn!("css file {} is not found, fallback to default", css_file.display());
            }
        }
        if !user_js_file.is_empty() {
            let user_js_file = PathBuf::from(user_js_file);
            if user_js_file.exists() {
                js_file = user_js_file;
            } else {
                log::warn!("js file {} is not found, fallback to default", js_file.display());
            }
        }

        Self {
            receiver,
            config: PreviewerConfig::new(
                client.eval("g:nvim_previewer_browser"),
                client.eval("g:nvim_previewer_port"),
                css_file,
                js_file,
            ),
            client: RefCell::new(client),
            logdir: cachedir.join("logs"),
            cachedir,
        }
    }

    pub fn recv(&self) -> &Receiver<(String, Vec<Value>)> {
        &self.receiver
    }

    fn preview(&self, params: Vec<Value>) -> Result<()> {
        let mut path = PREVIEW_FILE_PATH.lock().unwrap();
        *path = params.get(0)
            .ok_or(anyerr!("file path is not provided"))?
            .as_str().map(|v| v.to_owned());

        let url = format!("http://{DEFUALT_HOST}:{}", self.config.port);
        let r = if let Some(browser) = &self.config.browser {
            open::with(url, browser)
        } else {
            open::that(url)
        };
        if let Err(e) = r {
            self.client.borrow_mut().print(format!("failed to start browser: {e:?}"));
        }

        Ok(())
    }

    pub fn print<S: AsRef<str>>(&self, msg: S) {
        self.client.borrow_mut().print(msg.as_ref());
    }
}

fn main() {
    let previewer = Previewer::new(nvim_agent::new_client());

    let file_appender = tracing_appender::rolling::daily(previewer.logdir.as_path(), PKG_VERSION);
    let (non_blocking_appender, _guard) = tracing_appender::non_blocking(file_appender);
    tracing_subscriber::fmt().with_writer(non_blocking_appender.make_writer()).init();

    let config = previewer.config.clone();
    std::thread::spawn(move || {
        if let Err(e) = server(config) {
            log::error!("start server failed: {e:?}");
        }
    });
    log::info!("server started with configuration: {}", previewer.config);

    for (event, params) in previewer.recv() {
        let r = match event.as_str() {
            "preview" => {
                previewer.preview(params).map_err(|e| format!("oops, {e:?}"))
            }
            _ => {
                Err("unknown command".to_owned())
            }
        };
        if let Err(e) = r {
            previewer.print(e)
        }
    }
}
