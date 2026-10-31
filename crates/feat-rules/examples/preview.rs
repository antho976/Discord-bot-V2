//! `cargo run -p feat-rules --example preview -- <port>`: this feature's dashboard pages with
//! sample data and no Discord connection.
fn main() -> bot_core::anyhow::Result<()> {
    bot_core::preview::main(vec![std::sync::Arc::new(feat_rules::Rules::default())])
}
