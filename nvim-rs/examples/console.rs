use tokio::net::TcpListener;
use tokio::io::{self, AsyncReadExt, AsyncWriteExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
   let stdin = io::stdin();
   let mut reader = io::BufReader::new(stdin);
   loop {
      let mut buf = [0; 1024];
       let n = match reader.read(&mut buf).await {
           Ok(n) if n == 0 => return Ok(()),
           Ok(n) => n,
           Err(e) => {
               eprintln!("failed to read from socket; err = {:?}", e);
               return Ok(());
           }
       };
       let content = &buf[0..n];
       println!("got request {:x?}", content);
   }
}
