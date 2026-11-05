//! The Server page, at `/server`: the latest reading on top, then CPU and memory over 24 hours
//! or 7 days, for the machine and for the bot. Nothing here is about one Discord server; a
//! server in the shell is only there to draw the sidebar.

use bot_core::axum::Router;
use bot_core::axum::extract::{Query, State};
use bot_core::axum::response::IntoResponse;
use bot_core::axum::routing::get;
use bot_core::web::ui::{self, Icon};
use bot_core::web::{Level, User, WebError, WebResult};
use bot_core::{Ctx, now_ms};
use maud::{Markup, PreEscaped, html};

use crate::probe::Scope;
use crate::store::{self, Sample};
use crate::{KEY, machine};

pub fn routes() -> Router<Ctx> {
    Router::new().route("/server", get(page))
}

#[derive(serde::Deserialize)]
struct PageQuery {
    range: Option<String>,
}

const MINUTE: i64 = 60_000;

async fn page(State(ctx): State<Ctx>, user: User, Query(q): Query<PageQuery>) -> WebResult {
    let guild = ctx.guilds_for(&user).first().copied().ok_or(WebError::NotFound)?;
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let week = q.range.as_deref() == Some("7d");
    let now = now_ms();
    let latest = store::latest(&ctx)?;
    let rows = store::since(&ctx, now - if week { store::WEEK_MS } else { 86_400_000 })?;
    // 288 points for a day, 672 for a week: enough for a spike to show, few enough to draw.
    let bucket = if week { 15 * MINUTE } else { 5 * MINUTE };
    let charted: Vec<_> = rows.iter().map(store::charted).collect();
    let points = store::downsample(&charted, bucket);
    let label = |at: i64| page.user.local(at).strftime(if week { "%a %H:%M" } else { "%H:%M" }).to_string();
    let m = machine();
    let total_mem = latest.and_then(|s| s.mem_total);
    let span = if week { "7 days" } else { "24 hours" };

    let body = html! {
        style { (PreEscaped(CSS)) }
        @match &latest {
            None => {
                section.sv-panel {
                    (ui::empty(Icon::Server, "No readings yet", "The bot reads the server once a minute, so the first numbers show up a minute after it starts."))
                }
            }
            Some(s) => {
                (now_panel(s, &rows, &m, span, now, &|at| page.user.local(at).strftime("%b %-d").to_string()))
                div.sv-over #sv-over {
                    div.sv-over-head {
                        h2 { "Over the last " (span) }
                        div.seg #sv-range {
                            a.on[!week] href="/server" data-swap="#sv-over" { "24 hours" }
                            a.on[week] href="/server?range=7d" data-swap="#sv-over" { "7 days" }
                        }
                    }
                    @if points.len() < 2 {
                        div.sv-panel {
                            (ui::empty(Icon::Chart, "The charts start in a few minutes", if week { "Each point is 15 minutes of readings, so the lines start once there are two." } else { "Each point is 5 minutes of readings, so the lines start once there are two." }))
                        }
                    } @else {
                        div.sv-charts {
                            // CPU scales to its peak, but never under 10% (1% for the bot), so an
                            // idle hour doesn't look like a storm. Memory scales to all there is.
                            (chart(&points, 0, &label, "Server CPU", format!("All {} cores", m.cores), Some(1000), &pct, span))
                            (chart(&points, 1, &label, "Server memory", total_mem.map_or("In use".into(), |t| format!("In use, of {}", kib(t))), total_mem, &kib, span))
                            (chart(&points, 2, &label, "Bot CPU", format!("{} · share of all {} cores", scope_name(m.scope).0, m.cores), Some(100), &pct, span))
                            (chart(&points, 3, &label, "Bot memory", scope_name(m.scope).1.to_owned(), None, &kib, span))
                        }
                    }
                }
                p.sv-foot { "Per-container detail and alerts: " a href="http://server1" rel="noreferrer" { "Beszel at http://server1" } ", over Tailscale." }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Server", None, body).into_response())
}

/// Who the bot's numbers cover, short and long: in the image Caddy and Relay share its container.
fn scope_name(scope: Scope) -> (&'static str, &'static str) {
    match scope {
        Scope::Container => ("Bot, Caddy and Relay", "Bot, Caddy and Relay, in one container"),
        Scope::Process => ("The bot's process", "The bot's own process"),
    }
}

/// The latest reading: CPU, memory, disk, uptime, and the temperature when there's a sensor.
fn now_panel(s: &Sample, rows: &[Sample], m: &crate::Machine, span: &str, now: i64, day: &dyn Fn(i64) -> String) -> Markup {
    let share = |used: i64, total: i64| if total > 0 { (used * 1000 / total).clamp(0, 1000) } else { 0 };
    let hot_peak = rows.iter().filter_map(|r| r.temp).max();
    html! {
        section.sv-panel.sv-now {
            div.sv-now-head {
                h2 { @if let Some(name) = &m.cpu_model { (name) " · " } (m.cores) " cores" }
                span.muted { "Read " (bot_core::text::ago(s.at)) @if now - s.at > 3 * MINUTE { ", nothing since" } }
            }
            div.sv-figs {
                (figure("CPU", s.cpu.map(pct), s.load1.map(|l| format!("Load {:.2}", l as f64 / 100.0)), s.cpu.map(|c| c / 10)))
                (figure("Memory used", s.mem_used.map(kib), s.mem_used.zip(s.mem_total).map(|(u, t)| format!("of {} · {}%", kib(t), share(u, t) / 10)), s.mem_used.zip(s.mem_total).map(|(u, t)| share(u, t))))
                (figure("Disk used", s.disk_used.map(mib), s.disk_used.zip(s.disk_total).map(|(u, t)| format!("of {} · {}%", mib(t), share(u, t) / 10)), s.disk_used.zip(s.disk_total).map(|(u, t)| share(u, t))))
                (figure("Server up", s.uptime.map(uptime), s.uptime.map(|u| format!("Since {}", day(s.at - u * 1000))), None))
                @if let Some(t) = s.temp {
                    (figure("CPU temperature", Some(deg(t)), hot_peak.map(|p| format!("Peak {} in {span}", deg(p))), None))
                }
            }
        }
    }
}

/// One reading: the figure, what it is, one detail, and a meter (tenths of a percent) when it's
/// a share of something. A reading the bot couldn't take says so instead of showing a figure.
fn figure(label: &str, value: Option<String>, detail: Option<String>, meter: Option<i64>) -> Markup {
    html! {
        div.sv-fig {
            @match value {
                Some(v) => b { (v) },
                None => b.sv-unread { "Not readable" },
            }
            span.sv-fig-label { (label) }
            @if let Some(d) = detail { span.sv-fig-detail { (d) } }
            @if let Some(p) = meter {
                span.meter.sv-meter.high[p >= 900] role="img" aria-label={ (p / 10) "%" } { span style={ "width:" (format!("{:.1}", p as f64 / 10.0)) "%" } {} }
            }
        }
    }
}

/// One chart panel: the name, what it covers, the average and peak, and the line.
#[allow(clippy::too_many_arguments)]
fn chart(points: &[(i64, store::Charted)], i: usize, label: &dyn Fn(i64) -> String, title: &str, about: String, top: Option<i64>, show: &dyn Fn(i64) -> String, span: &str) -> Markup {
    let series: Vec<(String, i64)> = points.iter().filter_map(|(at, v)| v[i].map(|v| (label(*at), v))).collect();
    let peak = series.iter().map(|p| p.1).max();
    let avg = (!series.is_empty()).then(|| series.iter().map(|p| p.1).sum::<i64>() / series.len() as i64);
    html! {
        section.sv-panel.sv-chart {
            div.sv-chart-head {
                div.grow {
                    h3 { (title) }
                    p { (about) }
                }
                @if let (Some(a), Some(p)) = (avg, peak) {
                    div.sv-chart-fig { b { (show(a)) } span { "average · peak " (show(p)) } }
                }
            }
            @if series.len() < 2 {
                p.sv-none { "Nothing read in the last " (span) "." }
            } @else {
                (ui::area_chart_readings(&series, &format!("{title} over the last {span}: average {}, peak {}", show(avg.unwrap_or(0)), show(peak.unwrap_or(0))), top, show))
            }
        }
    }
}

/// Hundredths of a percent: "0.08%", "7.4%", "62%".
fn pct(v: i64) -> String {
    match v {
        ..10 => format!("{:.2}%", v as f64 / 100.0),
        10..1000 => format!("{:.1}%", v as f64 / 100.0),
        _ => format!("{}%", (v + 50) / 100),
    }
}

/// KiB as "820 MB" or "3.6 GB" (the 1024 kind, as free and top count them).
fn kib(v: i64) -> String {
    let mb = v as f64 / 1024.0;
    if mb < 1000.0 { format!("{} MB", mb.round() as i64) } else { format!("{:.1} GB", mb / 1024.0) }
}

/// MiB as "62 GB", "8.4 GB" or "640 MB".
fn mib(v: i64) -> String {
    let gb = v as f64 / 1024.0;
    if v < 1000 {
        format!("{v} MB")
    } else if gb < 10.0 {
        format!("{gb:.1} GB")
    } else {
        format!("{} GB", gb.round() as i64)
    }
}

/// Tenths of a degree as "46 °C".
fn deg(v: i64) -> String {
    format!("{} °C", (v + 5) / 10)
}

/// Seconds as "40 min", "5 h" or "12 days".
fn uptime(secs: i64) -> String {
    match secs {
        ..3600 => format!("{} min", secs.max(0) / 60),
        3600..172_800 => format!("{} h", secs / 3600),
        _ => format!("{} days", secs / 86_400),
    }
}

const CSS: &str = r#"
/* Server: the house style's bordered panels, figures without tiles, lilac for the lines and
   meters, amber only on a meter that's nearly full. Classes are prefixed sv-. */
.sv-panel { background: var(--raised); border: 1px solid var(--edge-1); border-radius: 14px; padding: 18px 20px; min-width: 0; }
.sv-now { display: flex; flex-direction: column; gap: 16px; }
.sv-now-head, .sv-over-head, .sv-chart-head { display: flex; align-items: baseline; justify-content: space-between; gap: 8px 16px; flex-wrap: wrap; }
.sv-now-head h2, .sv-over-head h2 { font-size: 14px; font-weight: 500; }
.sv-now-head .muted { font-size: 12px; }
.sv-figs { display: grid; grid-template-columns: repeat(auto-fit, minmax(150px, 1fr)); gap: 16px 24px; }
.sv-fig { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.sv-fig b { font: 600 26px/1.15 var(--sans); letter-spacing: -0.02em; font-variant-numeric: tabular-nums; white-space: nowrap; }
.sv-fig b.sv-unread { font-size: 18px; line-height: 1.6; color: var(--ink-3); }
.sv-fig-label { font-size: 13px; color: var(--ink-2); }
.sv-fig-detail { font-size: 12px; color: var(--ink-3); font-variant-numeric: tabular-nums; }
.sv-meter { margin-top: 8px; height: 4px; max-width: 220px; }
.sv-meter.high > span { background: var(--waiting); }
.sv-over { display: flex; flex-direction: column; gap: 14px; }
.sv-over-head { align-items: center; }
.sv-charts { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 16px; }
.sv-chart { display: flex; flex-direction: column; gap: 14px; }
.sv-chart-head { align-items: flex-start; }
.sv-chart-head h3 { font-size: 14px; font-weight: 500; margin: 0; }
.sv-chart-head p { font-size: 12px; color: var(--ink-3); margin: 2px 0 0; }
.sv-chart-fig { display: flex; flex-direction: column; align-items: flex-end; gap: 1px; text-align: right; }
.sv-chart-fig b { font-size: 18px; font-weight: 600; font-variant-numeric: tabular-nums; line-height: 1.25; }
.sv-chart-fig span { font-size: 12px; color: var(--ink-3); font-variant-numeric: tabular-nums; }
.sv-chart .area svg { height: 150px; }
.sv-none { font-size: 13px; color: var(--ink-3); padding: 40px 0; text-align: center; }
.sv-foot { font-size: 12px; color: var(--ink-3); }
.sv-foot a { color: var(--ink-2); text-decoration: underline; text-decoration-color: var(--edge-3); text-underline-offset: 3px; }
.sv-foot a:hover { color: var(--ink); }
@container col (max-width: 760px) {
  .sv-charts { grid-template-columns: minmax(0, 1fr); }
}
@container col (max-width: 420px) {
  .sv-figs { grid-template-columns: repeat(2, minmax(0, 1fr)); }
  .sv-fig b { font-size: 22px; }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::reqwest;

    async fn get(web: &reqwest::Client, addr: std::net::SocketAddr, cookie: &str, path: &str) -> (u16, String) {
        let res = web.get(format!("http://{addr}{path}")).header("cookie", cookie).send().await.unwrap();
        (res.status().as_u16(), res.text().await.unwrap())
    }

    /// The page in the preview, signed in as the owner: nothing read yet, the first minutes, a
    /// reading that couldn't take everything, then a week.
    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn page_draws_every_state() {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.install_features(vec![std::sync::Arc::new(crate::Server::default())]).unwrap();
        bot_core::preview::seed(&ctx).unwrap();
        ctx.db.with(|c| c.execute("DELETE FROM server_samples", [])).unwrap();
        let listener = bot_core::tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = bot_core::web::router(ctx.clone());
        bot_core::tokio::spawn(async move { bot_core::axum::serve(listener, app).await });
        let web = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
        let login = web.get(format!("http://{addr}/dev/login?to=/server")).send().await.unwrap();
        let cookie = login.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_owned();

        let (status, html) = get(&web, addr, &cookie, "/server").await;
        assert_eq!(status, 200);
        assert!(html.contains("No readings yet"), "{html}");

        let now = now_ms();
        let first = Sample { at: now - MINUTE, cpu: Some(420), mem_used: Some(3_000_000), mem_total: Some(16_000_000), uptime: Some(90), ..Sample::default() };
        store::record(&ctx, &first).unwrap();
        let (_, html) = get(&web, addr, &cookie, "/server").await;
        assert!(html.contains("4.2%") && html.contains("The charts start in a few minutes"), "{html}");
        // No disk reading: said, not drawn as zero. No sensor: no temperature at all.
        assert!(html.contains("Not readable") && !html.contains("CPU temperature"), "{html}");

        store::seed(&ctx, now).unwrap();
        let (_, html) = get(&web, addr, &cookie, "/server").await;
        assert!(html.contains("Server CPU") && html.contains("Bot memory") && html.contains("figure class=\"area\""), "{html}");
        assert!(html.contains("Over the last 24 hours") && html.contains("CPU temperature"));
        let (_, html) = get(&web, addr, &cookie, "/server?range=7d").await;
        assert!(html.contains("Over the last 7 days") && html.contains("Peak 77 °C in 7 days"), "{html}");
        // The account menu links it, and the Features page doesn't offer a switch for it.
        assert!(html.contains("href=\"/server\""));
        let (_, html) = get(&web, addr, &cookie, &format!("/g/{}/features", bot_core::preview::GUILD_A)).await;
        assert!(!html.contains("features/server"), "{html}");
    }

    #[test]
    fn figures_read_plainly() {
        assert_eq!(pct(8), "0.08%");
        assert_eq!(pct(46), "0.5%");
        assert_eq!(pct(744), "7.4%");
        assert_eq!(pct(6150), "62%");
        assert_eq!(kib(120 * 1024), "120 MB");
        assert_eq!(kib(3_774_874), "3.6 GB");
        assert_eq!(mib(62_900), "61 GB");
        assert_eq!(mib(640), "640 MB");
        assert_eq!(deg(455), "46 °C");
        assert_eq!(uptime(2_400), "40 min");
        assert_eq!(uptime(5 * 3600 + 10), "5 h");
        assert_eq!(uptime(12 * 86_400 + 7), "12 days");
    }
}
