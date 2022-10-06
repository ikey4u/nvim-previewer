use std::fs::File;
use std::io::Read;
use std::sync::Mutex;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::cell::RefCell;

use nvim_rs::Value;
use nvim_rs::NeovimClient;
use anyhow::Result;
use anyhow::Context;
use tracing_subscriber::fmt::writer::MakeWriter;
use axum::response::{Response, IntoResponse};
use http::status::StatusCode;
use once_cell::sync::Lazy;
use comrak::{markdown_to_html, ComrakOptions};

const DEFAULT_PORT: u16 = 3008;
static PREVIEW_FILE_PATH: Lazy<Arc<Mutex<Option<String>>>> = Lazy::new(|| {
    Arc::new(Mutex::new(None))
});

fn server<'a>(config: &'a PreviewerConfig) -> Result<()> {
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], config.port));
    log::info!("web server start to listen at {}", addr.to_string());
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(5)
        .enable_all()
        .build()?;
    let r = rt.block_on(async {
        let app = axum::Router::new().route("/", axum::routing::get(render));
        axum::Server::bind(&addr)
            .serve(app.into_make_service())
            .await.unwrap();
    });
    log::info!("web server exit with result: ${r:?}");
    Ok(())
}

async fn render() -> impl IntoResponse {
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

            <style type="text/css"> {css} </style>
          </head>

          <body>
            {body}
          </body>

        </html>
    "#,
        title = "Previewer",
        css = "",
        body = html,
    );
    Response::builder().status(StatusCode::OK)
        .header(http::header::CONTENT_TYPE, http::HeaderValue::from_str("text/html").unwrap())
        .body(axum::body::boxed(axum::body::Full::from(html_template)))
        .unwrap()
}

fn main() {
    let file_appender = tracing_appender::rolling::daily(".", ".logs/nvim-rs.log");
    let (non_blocking_appender, _guard) = tracing_appender::non_blocking(file_appender);
    tracing_subscriber::fmt().with_writer(non_blocking_appender.make_writer()).init();

    let client = nvim_rs::new_client();
    let previewer = Previewer::new(client);
    let config = previewer.config.clone();
    std::thread::spawn(move || {
        if let Err(e) = server(&config) {
            log::error!("start server failed: {e:?}");
        }
    });
    log::info!("server started with configuration: {:?}", previewer.config);

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

impl PreviewerConfig {
    pub fn new<S1: AsRef<str>, S2: AsRef<str>>(browser: S1, port: S2) -> Self {
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
        config
    }
}

struct Previewer {
    client: RefCell<NeovimClient>,
    config: PreviewerConfig,
    receiver: Receiver<(String, Vec<Value>)>,
}

impl Previewer {
    pub fn new(mut client: NeovimClient) -> Self {
        let receiver = client.start();
        Self {
            receiver,
            config: PreviewerConfig::new(
                client.eval("g:nvim_previewer_browser"),
                client.eval("g:nvim_previewer_port"),
            ),
            client: RefCell::new(client),
        }
    }

    pub fn recv(&self) -> &Receiver<(String, Vec<Value>)> {
        &self.receiver
    }

    fn preview(&self, params: Vec<Value>) -> Result<()> {
        let mut path = PREVIEW_FILE_PATH.lock().unwrap();
        *path = params.get(0).context("file path is not provided")?.as_str().map(|v| v.to_owned());

        let url = format!("http://127.0.0.1:{}", self.config.port);
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
