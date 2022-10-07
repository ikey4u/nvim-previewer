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

use axum::extract::Path as AxumPath;
use nvim_rs::Value;
use nvim_rs::NeovimClient;
use anyhow::Result;
use anyhow::Context;
use tracing_subscriber::fmt::writer::MakeWriter;
use axum::response::Response;
use axum::response::IntoResponse;
use axum::extract::Extension;
use http::status::StatusCode;
use once_cell::sync::Lazy;
use comrak::markdown_to_html;
use comrak::ComrakOptions;

const DEFAULT_PORT: u16 = 3008;
const DEFUALT_HOST: &'static str = "127.0.0.1";
const PKG_VERSION: &'static str = env!("CARGO_PKG_VERSION");
const PKG_NAME: &'static str = env!("CARGO_PKG_NAME");
static PREVIEW_FILE_PATH: Lazy<Arc<Mutex<Option<String>>>> = Lazy::new(|| {
    Arc::new(Mutex::new(None))
});

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
            .route("/file/:name", axum::routing::get(file))
            .layer(Extension(config));
        axum::Server::bind(&addr)
            .serve(app.into_make_service())
            .await.unwrap();
    });
    log::info!("web server exit with result: {r:?}");
    Ok(())
}

async fn file(Extension(config): Extension<Arc<PreviewerConfig>>, AxumPath(name): AxumPath<String>) -> impl IntoResponse {
    let mut mime = "text/plain";
    let content = match name.as_str() {
        "css" => {
            let mut content = String::new();
            mime = "text/css";
            if let Ok(mut f) = File::open(config.css_file.as_path()) {
                _ = f.read_to_string(&mut content);
            }
            content
        }
        "js" => {
            mime = "text/javascript";
            let mut content = String::new();
            if let Ok(mut f) = File::open(config.js_file.as_path()) {
                _ = f.read_to_string(&mut content);
            }
            content
        }
        _ => "".to_owned()
    };
    Response::builder().status(StatusCode::OK)
        .header(http::header::CONTENT_TYPE, http::HeaderValue::from_str(mime).unwrap())
        .body(axum::body::boxed(axum::body::Full::from(content)))
        .unwrap()
}

async fn render(Extension(_config): Extension<Arc<PreviewerConfig>>) -> impl IntoResponse {
    let html = match PREVIEW_FILE_PATH.lock().unwrap().as_ref() {
        Some(path) => {
            log::info!("start to render file: {path}");
            if let Ok(mut f) = File::open(path) {
                let mut content = String::new();
                _ = f.read_to_string(&mut content);
                markdown_to_html(&content, &ComrakOptions::default())
            } else {
                format!("failed to open file: {path}")
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
                <link rel="stylesheet" type="text/css" href="/file/css">
            </head>
            <body class="nvim-previewer">
                <article class="markdown-body">
                    {body}
                </article>
                <script src="/file/js"></script>
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
    let previewer = Previewer::new(nvim_rs::new_client());

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
