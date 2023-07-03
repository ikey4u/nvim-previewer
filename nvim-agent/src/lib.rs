mod client;
mod rpc;

use std::io;

pub use client::NeovimApi;
use errlog::logmsg;
pub use rmpv::Value;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("oops, error happens: {0}")]
    Dirty(String),
    #[error("rmpv decode error: {0:?}")]
    RmpvDecode(#[from] rmpv::decode::Error),
    #[error("anyhow error: {0:?}")]
    Anyhow(#[from] errlog::Error),
    #[error("io error: {0:?}")]
    IoError(#[from] std::io::Error),
}

pub type Result<T> = errlog::Result<T, Error>;
pub type NeovimClient = client::Client<io::Stdin, io::Stdout>;

impl Error {
    pub fn new<S: AsRef<str>>(msg: S) -> Self {
        Error::Dirty(msg.as_ref().to_owned())
    }
}

pub fn new_client() -> NeovimClient {
    client::Client::new(io::stdin(), io::stdout())
}

impl NeovimClient {
    /// evaluate a vim expression `expr` and return the value as string (if the value is a string
    /// within single or double quote, the quote will be removed), if the return value is empty,
    /// then some errors happens or the result is empty.
    pub fn eval<S: AsRef<str>>(&mut self, expr: S) -> String {
        match self.nvim_eval(expr.as_ref().to_owned()) {
            Ok(v) => {
                let v = v.to_string();
                let v = v.trim().trim_matches(|c| c == '\'' || c == '"');
                v.to_owned()
            }
            Err(e) => {
                logmsg!(
                    ERROR,
                    "failed to evaluation expresson {} with error {:?}",
                    expr.as_ref(),
                    e
                );
                "".to_owned()
            }
        }
    }

    /// print message in neovim
    pub fn print<S: AsRef<str>>(&mut self, msg: S) {
        _ = self.nvim_command(format!("echo '{}'", msg.as_ref()));
    }
}
