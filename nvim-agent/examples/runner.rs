use tracing_subscriber::{self, fmt::writer::MakeWriter};
use nvim_agent::NeovimApi;

fn main() {
    let file_appender = tracing_appender::rolling::daily(".", ".logs/nvim-agent.log");
    let (non_blocking_appender, _guard) = tracing_appender::non_blocking(file_appender);
    tracing_subscriber::fmt().with_writer(non_blocking_appender.make_writer()).init();

    log::info!("runner started");
    let mut client = nvim_agent::new_client();
    for (event, params) in client.start() {
        match event.as_str() {
            "run" => {
                log::info!("plugin receive: {event} {params:?}");
                client.print(format!("I got: {params:?}!"))
            }
            _ => {
                log::info!("unknown command ...");
            }
        }
    }
}
