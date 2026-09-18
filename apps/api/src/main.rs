use std::env;
use std::error::Error;
use std::net::SocketAddr;

use taskharbor_api::app;
use tokio::net::TcpListener;

const DEFAULT_BIND_ADDR: &str = "127.0.0.1:3000";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let bind_addr =
        env::var("TASKHARBOR_BIND_ADDR").unwrap_or_else(|_| DEFAULT_BIND_ADDR.to_owned());
    let bind_addr = bind_addr.parse::<SocketAddr>()?;
    let listener = TcpListener::bind(bind_addr).await?;

    println!("TaskHarbor API listening on http://{bind_addr}");
    axum::serve(listener, app()).await?;

    Ok(())
}
