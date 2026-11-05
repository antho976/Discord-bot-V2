//! `cargo run -p feat-server --example preview -- <port>`: the Server page with a sample week of
//! readings and no Discord connection. Open `http://localhost:<port>/dev/login?to=/server`.
fn main() -> bot_core::anyhow::Result<()> {
    bot_core::preview::main(vec![std::sync::Arc::new(feat_server::Server::default())])
}
