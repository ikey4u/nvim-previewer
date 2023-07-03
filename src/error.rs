use axum::response::{IntoResponse, Response};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("{0:?}")]
    Other(#[from] anyhow::Error),
}

impl Error {
    pub fn new<S: AsRef<str>>(msg: S) -> Self {
        Self::Other(anyhow::Error::msg(msg.as_ref().to_owned()))
    }
}

#[macro_export]
macro_rules! anyerr {
    ($msg:literal $(,)?) => {
        anyhow::anyhow!(format!("[{}].[{}]: {}", file!(), line!(), format!($msg)))
    };
    ($fmt:expr, $($arg:tt)*) => {
        anyhow::anyhow!(format!("[{}].[{}]: {}", file!(), line!(), format!($fmt, $($arg)*)))
    };
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (axum::http::status::StatusCode::OK, format!("{self:?}"))
            .into_response()
    }
}
