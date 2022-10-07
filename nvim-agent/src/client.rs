use std::{io::{Read, Write, BufReader, BufWriter}, sync::{mpsc, Arc, Mutex}, collections::HashMap};

use crate::{Result, Error};
use crate::Value;
use crate::rpc::Message;

use errlog::logmsg;

include!(concat!(env!("OUT_DIR"), concat!("/", "nvim_api.rs")));

pub struct Client<R: Read + Send + 'static, W: Write + Send + 'static> {
    msgid: u64,
    reader: Arc<Mutex<BufReader<R>>>,
    writer: Arc<Mutex<BufWriter<W>>>,
    tasks: Arc<Mutex<HashMap<u64, mpsc::Sender<Result<Value>>>>>,
}

impl<R: Read + Send + 'static, W: Write + Send + 'static> Client<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Client {
            msgid: 0,
            reader: Arc::new(Mutex::new(BufReader::new(reader))),
            writer: Arc::new(Mutex::new(BufWriter::new(writer))),
            tasks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// connect to an exist neovim instance by stdin and stdout
    pub fn start(&self) -> mpsc::Receiver<(String, Vec<Value>)> {
        let (tx, rx) = mpsc::channel();
        let reader = self.reader.clone();
        let writer = self.writer.clone();
        let senders = self.tasks.clone();

        std::thread::spawn(move || {
            loop {
                let reader = &mut *reader.lock().unwrap();
                match Message::read_from(reader) {
                    Ok(Message::Request { msgid, method, params }) => {
                        logmsg!(DEBUG, "RpcRequest: {method}");
                        let resp = Message::Response { msgid, result: Value::Nil, error: Value::Nil };
                        let writer = &mut *writer.lock().unwrap();
                        resp.write_to(writer).expect("failed to send response");
                    }
                    Ok(Message::Response { msgid, error, result }) => {
                        logmsg!(DEBUG, "RpcResponse: {:?}, result {:?}", error, result);
                        let sender = senders.lock().unwrap().remove(&msgid).unwrap();
                        let r = if error != Value::Nil {
                            sender.send(Err(Error::Dirty(format!("{error:?}"))))
                        } else {
                            sender.send(Ok(result))
                        };
                        if let Err(e) = r {
                            logmsg!(ERROR, "cannot reply to RpcResponse: {:?}", e)
                        }
                    }
                    Ok(Message::Notify { method, params }) => {
                        logmsg!(DEBUG, "RpcNotify: {} {:?}", method, params);
                        if let Err(e) = tx.send((method, params)) {
                            logmsg!(ERROR, "failed to transmit notifications: {:?}", e);
                        }
                    }
                    Err(e) => {
                        logmsg!(ERROR, "read error: {:?}", e);
                        break;
                    }
                }
            }
        });

        rx
    }
}
