//! The comm server binary without plugins (see cli.rs).

#[tokio::main]
async fn main() {
    andruavserver::cli::run(|_| {}).await;
}
