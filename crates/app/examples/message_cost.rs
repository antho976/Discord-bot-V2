//! What one ordinary member message costs the bot, per feature: the path every message takes.
//!
//! Builds the preview's sample servers (members, channels, settings) in memory, then hands
//! synthetic messages from members to every feature's `on_event`, the way the gateway's dispatch
//! does, and times each feature. No Discord: the messages are plain chat (no links, no
//! commands), so nothing should need the network; a feature that tries shows as a slow outlier.
//!
//!     cargo run --release -p bot --example message_cost [-- messages]

use std::sync::Arc;
use std::time::{Duration, Instant};

use bot_core::twilight_model::gateway::event::Event;
use bot_core::twilight_model::gateway::payload::incoming::MessageCreate;
use bot_core::{Ctx, Feature, preview, serde_json};

fn features() -> Vec<Arc<dyn Feature>> {
    vec![
        Arc::new(feat_levels::Levels::default()),
        Arc::new(feat_alerts::Alerts),
        Arc::new(feat_guard::Guard::default()),
        Arc::new(feat_connect::Connect),
        Arc::new(feat_pets::Pets),
        Arc::new(feat_welcome::Welcome::default()),
        Arc::new(feat_audit::Audit::default()),
        Arc::new(feat_community::Community),
        Arc::new(feat_tickets::Tickets::default()),
        Arc::new(feat_utility::Utility::default()),
        Arc::new(feat_embeds::Embeds),
        Arc::new(feat_modmail::Modmail::default()),
        Arc::new(feat_rules::Rules::default()),
        Arc::new(feat_commands::Commands::default()),
        Arc::new(feat_reviews::Reviews),
    ]
}

const LINES: &[&str] = &[
    "anyone up for the world 6 push tonight?",
    "lol same",
    "gg, that boss took forever",
    "does anyone know when the next stream is? I missed the schedule post",
    "thanks! 🙏",
    "I finally got the Cryosnake pet after like 300 tries, this game is something else honestly",
];

fn message(id: u64, channel: u64, user: (u64, &str), text: &str) -> Event {
    let v = serde_json::json!({
        "id": id.to_string(), "channel_id": channel.to_string(), "guild_id": preview::GUILD_A.to_string(),
        "content": text,
        "author": {"id": user.0.to_string(), "username": user.1, "discriminator": "0", "avatar": null},
        "member": {"roles": [], "joined_at": "2025-01-01T00:00:00+00:00", "deaf": false, "mute": false, "flags": 0},
        "timestamp": "2026-11-01T10:00:00+00:00", "edited_timestamp": null, "tts": false,
        "mention_everyone": false, "mentions": [], "mention_roles": [], "attachments": [],
        "embeds": [], "pinned": false, "type": 0,
    });
    Event::MessageCreate(Box::new(MessageCreate(serde_json::from_value(v).expect("a valid message"))))
}

#[bot_core::tokio::main(flavor = "current_thread")]
async fn main() -> bot_core::anyhow::Result<()> {
    let n: usize = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(20_000);
    preview::ON.store(true, std::sync::atomic::Ordering::Relaxed);
    let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory()?)?;
    let feats = features();
    ctx.install_features(feats.clone())?;
    preview::seed(&ctx)?;
    let channel = ctx.text_channels(preview::GUILD_A).first().map(|c| c.0).expect("a text channel");
    let on: Vec<&Arc<dyn Feature>> = feats.iter().filter(|f| ctx.enabled(preview::GUILD_A, f.meta().key)).collect();

    let mut per: Vec<Duration> = vec![Duration::ZERO; on.len()];
    let mut whole: Vec<f64> = Vec::with_capacity(n);
    for i in 0..n {
        let user = preview::MEMBERS[i % preview::MEMBERS.len()];
        let ev = message(1_500_000_000_000_000_000 + i as u64, channel, user, LINES[i % LINES.len()]);
        ctx.cache.update(&ev);
        let t0 = Instant::now();
        for (k, f) in on.iter().enumerate() {
            let t = Instant::now();
            if let Err(e) = f.on_event(&ctx, &ev).await {
                eprintln!("{}: {e}", f.meta().key);
            }
            per[k] += t.elapsed();
        }
        whole.push(t0.elapsed().as_secs_f64() * 1e6);
    }
    whole.sort_by(f64::total_cmp);
    let p = |q: f64| whole[((whole.len() - 1) as f64 * q) as usize];
    println!("{n} messages, {} features on, {} members", on.len(), preview::MEMBERS.len());
    println!("per message: p50 {:.1} µs, p95 {:.1} µs, p99 {:.1} µs, max {:.1} µs", p(0.5), p(0.95), p(0.99), whole[whole.len() - 1]);
    let mut rows: Vec<(&str, Duration)> = on.iter().map(|f| f.meta().key).zip(per).collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.1));
    for (key, d) in rows {
        println!("  {key:<12} {:>8.2} µs/message", d.as_secs_f64() * 1e6 / n as f64);
    }
    Ok(())
}
