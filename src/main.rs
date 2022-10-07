use std::fmt::Display;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::cell::RefCell;
use std::net::SocketAddr;

use anyhow::Result;
use anyhow::Context;
use axum::response::Response;
use axum::response::IntoResponse;
use axum::extract::Extension;
use axum::extract::Query;
use axum::handler::Handler;
use axum::http;
use axum::http::status::StatusCode;
use comrak::Arena;
use comrak::nodes::NodeValue;
use comrak::parse_document;
use comrak::format_html;
use comrak::ComrakOptions;
use comrak::nodes::AstNode;
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
    let addr = format!("{DEFUALT_HOST}:{}", config.port).parse::<SocketAddr>()?;
    log::info!("web server start to listen at {}", addr.to_string());
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(5)
        .enable_all()
        .build()?;
    let r = rt.block_on(async {
        let app = axum::Router::new()
            .route("/", axum::routing::get(render))
            .route("/file", axum::routing::get(file))
            .fallback(fallback.into_service())
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

                let arena = Arena::new();
                let root = parse_document(&arena, &content, &ComrakOptions::default());
                markdown_hook(root, &|node| {
                    match &mut node.data.borrow_mut().value {
                        &mut NodeValue::Image(ref mut link) => {
                            let url = std::str::from_utf8(link.url.as_ref()).unwrap();
                            let local_filepath = filedir.join(url);
                            if local_filepath.exists() {
                                link.url = format!(
                                    "http://{DEFUALT_HOST}:{}/file?tag=path&val={}",
                                    config.port,
                                    local_filepath.display(),
                                ).as_bytes().to_vec();
                            }
                        }
                        _ => {}
                    }
                });

                let mut html = vec![];
                format_html(root, &ComrakOptions::default(), &mut html).unwrap();
                String::from_utf8(html).unwrap()
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
            </head>
            <body class="nvim-previewer">
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

fn markdown_hook<'a, F>(node: &'a AstNode<'a>, hook: &F)
where
    F: Fn(&'a AstNode<'a>)
{
    hook(node);
    for c in node.children() {
        markdown_hook(c, hook)
    }
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
        *path = params.get(0).context("file path is not provided")?.as_str().map(|v| v.to_owned());

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
