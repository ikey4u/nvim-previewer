use std::io::{Read, Write};

use crate::{Value, Result, Error};

use errlog::wraperr;
use rmpv::{decode::read_value, encode::write_value};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub enum Message {
    Request {
        msgid: u64,
        method: String,
        params: Vec<Value>,
    },
    Response {
        msgid: u64,
        error: Value,
        result: Value,
    },
    Notify {
        method: String,
        params: Vec<Value>,
    },
}

impl Message {
    /// Create message from reader
    ///
    /// Every RPC message is represented as an array of value, we read a array from the reader,
    /// then unpack them into the corresponding struct according to the type:
    ///
    /// - Request: [type: 0, msgid: Integer, method: String, params: Vec<Value>]
    /// - Response: [type: 1, msgid: Integer, error: Value, result: Value]
    /// - Notify: [type: 2, method: String, params: Vec<Value>]
    ///
    pub fn read_from<R: Read>(reader: &mut R) -> Result<Self> {
        let value = wraperr!(read_value(reader), "RPC reader is broken")?;
        let arr = wraperr!(value.as_array(), "RPC message must be an array")?;
        match wraperr!(arr.get(0).and_then(|v| v.as_i64()), "failed to get message type")? {
            0 => {
                let msgid = wraperr!(arr.get(1).and_then(|v| v.as_u64()), "failed to get message id")?;
                let method = wraperr!(arr.get(2).and_then(|v| v.as_str()), "failed to get message method")?;
                let params = wraperr!(arr.get(3).and_then(|v| v.as_array()), "failed to get message params")?;
                return Ok(Self::Request {
                    msgid,
                    method: method.to_owned(),
                    params: params.to_owned(),
                });
            }
            1 => {
                let msgid = wraperr!(arr.get(1).and_then(|v| v.as_u64()), "failed to get message id")?;
                let error = wraperr!(arr.get(2), "failed to get message error")?;
                let result = wraperr!(arr.get(3), "failed to get message result")?;
                return Ok(Self::Response {
                    msgid,
                    error: error.to_owned(),
                    result: result.to_owned(),
                });
            }
            2 => {
                let method = wraperr!(arr.get(1).and_then(|v| v.as_str()), "failed to get message method")?;
                let params = wraperr!(arr.get(2).and_then(|v| v.as_array()), "failed to get message params")?;
                return Ok(Self::Notify {
                    method: method.to_owned(),
                    params: params.to_owned(),
                });
            }
            _ => {
                return Err(Error::Dirty(format!("unknown message: {:?}", arr)));
            }
        }
    }

    /// Send message into writer
    pub fn write_to<W: Write>(&self, writer: &mut W) -> Result<()> {
        let mut value = vec![];
        match self {
            Message::Request { msgid, method, params } => {
                value.push(Value::from(0i32));
                value.push(Value::from(msgid.to_owned()));
                value.push(Value::from(method.to_owned()));
                value.push(Value::from(params.to_owned()));
            }
            Message::Response { msgid, error, result } => {
                value.push(Value::from(1i32));
                value.push(Value::from(msgid.to_owned()));
                value.push(Value::from(error.to_owned()));
                value.push(Value::from(result.to_owned()));
            }
            Message::Notify { method, params } => {
                value.push(Value::from(2i32));
                value.push(Value::from(method.to_owned()));
                value.push(Value::from(params.to_owned()));
            }
        }
        wraperr!(write_value(writer, &Value::from(value)), "failed to write mesage to writer")?;
        writer.flush()?;
        Ok(())
    }
}
