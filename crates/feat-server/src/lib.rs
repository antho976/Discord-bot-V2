//! Server: what the machine the bot runs on is doing (CPU, memory, disk, uptime, temperature)
//! and the bot's own share of it, read once a minute on the shared timer and kept for a week.
//! For the owner and Admin+ only, on one page that isn't tied to a Discord server.
//! Per-container detail and alerts stay with Beszel on the server itself.

pub mod probe;
pub mod store;
mod web;

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use bot_core::anyhow::Result;
use bot_core::{Ctx, Feature, FeatureMeta, Group, Icon, Migration, NavItem, feature_async, now_ms};

pub const KEY: &str = "server";

/// What doesn't change while the bot runs, read once at start (or set by the preview).
#[derive(Debug, Clone)]
pub struct Machine {
    /// "Intel Core i5-8500T".
    pub cpu_model: Option<String>,
    pub cores: u32,
    pub scope: probe::Scope,
}

static MACHINE: OnceLock<Machine> = OnceLock::new();

pub fn machine() -> Machine {
    MACHINE.get().cloned().unwrap_or(Machine { cpu_model: None, cores: 1, scope: probe::bot_scope() })
}

/// The previous minute's counters, for the CPU shares, and where things are read.
#[derive(Default)]
struct State {
    cpu: Option<probe::CpuTimes>,
    bot: Option<(u64, Instant)>,
    data_dir: PathBuf,
    temp: Option<PathBuf>,
}

#[derive(Default)]
pub struct Server {
    state: Mutex<State>,
}

#[feature_async]
impl Feature for Server {
    fn meta(&self) -> FeatureMeta {
        FeatureMeta {
            key: KEY,
            name: "Server",
            description: "CPU, memory and disk of the machine the bot runs on, and the bot's own share.",
            group: Group::Tools,
            nav: Vec::new(),
        }
    }

    fn migrations(&self) -> &'static [Migration] {
        store::MIGRATIONS
    }

    /// The machine is shared by every server, and its numbers are the owner's business.
    fn admin_plus_only(&self) -> bool {
        true
    }

    fn global_page(&self) -> Option<NavItem> {
        Some(NavItem { label: "Server", path: KEY, icon: Icon::Server, about: "The machine the bot runs on, read every minute and kept for a week." })
    }

    fn routes(&self) -> bot_core::axum::Router<Ctx> {
        web::routes()
    }

    /// Takes the first counters, so the first tick a minute later already has CPU shares.
    async fn start(&self, ctx: &Ctx) -> Result<()> {
        let scope = probe::bot_scope();
        let cpu = probe::cpu_times();
        let _ = MACHINE.set(Machine { cpu_model: probe::cpu_model(), cores: cpu.map_or(1, |c| c.1), scope });
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.cpu = cpu.map(|c| c.0);
        s.bot = probe::bot_cpu(scope);
        s.data_dir = probe::data_dir(&ctx.cfg.database_path);
        s.temp = probe::find_temp_sensor();
        Ok(())
    }

    async fn tick(&self, ctx: &Ctx) -> Result<()> {
        let sample = self.read();
        store::record(ctx, &sample)
    }

    fn seed_preview(&self, ctx: &Ctx, _guild: u64) -> Result<()> {
        let _ = MACHINE.set(Machine { cpu_model: Some("Intel Core i5-8500T".into()), cores: 6, scope: probe::Scope::Container });
        store::seed(ctx, now_ms())
    }
}

impl Server {
    /// This minute's reading. A few small file reads and one statvfs, all in microseconds.
    fn read(&self) -> store::Sample {
        let m = machine();
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let cpu_now = probe::cpu_times().map(|c| c.0);
        let cpu = st.cpu.zip(cpu_now).and_then(|(a, b)| probe::cpu_share(a, b));
        st.cpu = cpu_now;
        let bot_now = probe::bot_cpu(m.scope);
        let bot_cpu = st.bot.zip(bot_now).and_then(|(a, b)| probe::bot_share(a, b, m.cores));
        st.bot = bot_now;
        let mem = probe::memory();
        let disk = probe::disk(&st.data_dir);
        let mib = |b: u64| (b / (1024 * 1024)) as i64;
        store::Sample {
            at: now_ms(),
            cpu,
            mem_used: mem.map(|m| m.1 as i64),
            mem_total: mem.map(|m| m.0 as i64),
            disk_used: disk.map(|d| mib(d.0)),
            disk_total: disk.map(|d| mib(d.1)),
            load1: probe::load(),
            uptime: probe::uptime(),
            temp: st.temp.as_deref().and_then(probe::read_temp),
            bot_cpu,
            bot_mem: probe::bot_memory(m.scope).map(|k| k as i64),
        }
    }
}

#[cfg(test)]
mod tests {
    use bot_core::Feature;

    /// On the Linux machine running the tests, a reading has the machine's memory, disk and
    /// uptime, and the bot's memory, without a panic for whatever it couldn't read.
    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn reads_this_machine() {
        let ctx = bot_core::Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        let server = super::Server::default();
        server.start(&ctx).await.unwrap();
        let s = server.read();
        println!("{s:?} {:?}", super::machine());
        assert!(s.mem_total.is_some_and(|t| s.mem_used.is_some_and(|u| u > 0 && u <= t)));
        assert!(s.disk_total.is_some_and(|t| s.disk_used.is_some_and(|u| u <= t)));
        assert!(s.uptime.is_some_and(|u| u > 0));
        // Asked of this machine directly: `machine()` is set once per process, and a test that
        // seeds the preview may have set it first, to a container this machine may not be.
        assert!(super::probe::bot_memory(super::probe::bot_scope()).is_some_and(|m| m > 0));
    }
}
