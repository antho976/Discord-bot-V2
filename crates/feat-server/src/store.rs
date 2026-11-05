//! One row a minute, kept for a week. Every column is a small integer, `NULL` where that reading
//! couldn't be taken.

use bot_core::anyhow::Result;
use bot_core::rusqlite::{OptionalExtension, Row, params};
use bot_core::{Ctx, Migration};

pub const MIGRATIONS: &[Migration] = &[Migration {
    name: "001_samples",
    // at: ms since the epoch (the rowid, so a range by time is a range of the table).
    // cpu, bot_cpu: hundredths of a percent of the whole machine. mem_*, bot_mem: KiB.
    // disk_*: MiB. load1: hundredths. uptime: seconds. temp: tenths of a degree C.
    sql: "
        CREATE TABLE server_samples (
            at INTEGER PRIMARY KEY, cpu INTEGER, mem_used INTEGER, mem_total INTEGER,
            disk_used INTEGER, disk_total INTEGER, load1 INTEGER, uptime INTEGER, temp INTEGER,
            bot_cpu INTEGER, bot_mem INTEGER);
    ",
}];

pub const WEEK_MS: i64 = 7 * 86_400_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sample {
    pub at: i64,
    pub cpu: Option<i64>,
    pub mem_used: Option<i64>,
    pub mem_total: Option<i64>,
    pub disk_used: Option<i64>,
    pub disk_total: Option<i64>,
    pub load1: Option<i64>,
    pub uptime: Option<i64>,
    pub temp: Option<i64>,
    pub bot_cpu: Option<i64>,
    pub bot_mem: Option<i64>,
}

const COLUMNS: &str = "at, cpu, mem_used, mem_total, disk_used, disk_total, load1, uptime, temp, bot_cpu, bot_mem";

fn from_row(r: &Row) -> bot_core::rusqlite::Result<Sample> {
    Ok(Sample {
        at: r.get(0)?,
        cpu: r.get(1)?,
        mem_used: r.get(2)?,
        mem_total: r.get(3)?,
        disk_used: r.get(4)?,
        disk_total: r.get(5)?,
        load1: r.get(6)?,
        uptime: r.get(7)?,
        temp: r.get(8)?,
        bot_cpu: r.get(9)?,
        bot_mem: r.get(10)?,
    })
}

fn insert(c: &bot_core::rusqlite::Connection, s: &Sample) -> bot_core::rusqlite::Result<usize> {
    c.prepare_cached(&format!("INSERT OR REPLACE INTO server_samples ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"))?.execute(params![
        s.at, s.cpu, s.mem_used, s.mem_total, s.disk_used, s.disk_total, s.load1, s.uptime, s.temp, s.bot_cpu, s.bot_mem
    ])
}

/// Store this minute's reading and drop what's older than a week.
pub fn record(ctx: &Ctx, s: &Sample) -> Result<()> {
    ctx.db.with(|c| {
        insert(c, s)?;
        c.prepare_cached("DELETE FROM server_samples WHERE at <= ?1")?.execute([s.at - WEEK_MS])?;
        Ok(())
    })
}

pub fn latest(ctx: &Ctx) -> Result<Option<Sample>> {
    ctx.db.with(|c| c.query_row(&format!("SELECT {COLUMNS} FROM server_samples ORDER BY at DESC LIMIT 1"), [], from_row).optional())
}

/// Every reading since `since`, oldest first.
pub fn since(ctx: &Ctx, since: i64) -> Result<Vec<Sample>> {
    ctx.db.with(|c| {
        let mut s = c.prepare_cached(&format!("SELECT {COLUMNS} FROM server_samples WHERE at >= ?1 ORDER BY at"))?;
        s.query_map([since], from_row)?.collect()
    })
}

/// The values a chart draws, per reading: server CPU, server memory, bot CPU, bot memory.
pub type Charted = [Option<i64>; 4];

pub fn charted(s: &Sample) -> (i64, Charted) {
    (s.at, [s.cpu, s.mem_used, s.bot_cpu, s.bot_mem])
}

/// Averages over buckets of `bucket` ms, lined up on multiples of it so a bucket doesn't move
/// between page loads. Each value averages the readings that had it; a bucket with no reading
/// at all is left out (the bot was down) rather than drawn as zero. `rows` oldest first.
pub fn downsample<const N: usize>(rows: &[(i64, [Option<i64>; N])], bucket: i64) -> Vec<(i64, [Option<i64>; N])> {
    let mut out: Vec<(i64, [Option<i64>; N])> = Vec::new();
    let mut cur: Option<(i64, [(i64, i64); N])> = None;
    let flush = |b: i64, acc: &[(i64, i64); N], out: &mut Vec<(i64, [Option<i64>; N])>| {
        let mut v = [None; N];
        for (slot, (sum, n)) in v.iter_mut().zip(acc) {
            if *n > 0 {
                // Rounded to the nearest, halves away from zero.
                *slot = Some((2 * sum + n * sum.signum()) / (2 * n));
            }
        }
        out.push((b, v));
    };
    for (at, vals) in rows {
        let b = at.div_euclid(bucket) * bucket;
        match &mut cur {
            Some((cb, _)) if *cb == b => {}
            _ => {
                if let Some((cb, acc)) = &cur {
                    flush(*cb, acc, &mut out);
                }
                cur = Some((b, [(0, 0); N]));
            }
        }
        if let Some((_, acc)) = &mut cur {
            for (a, v) in acc.iter_mut().zip(vals) {
                if let Some(v) = v {
                    a.0 += v;
                    a.1 += 1;
                }
            }
        }
    }
    if let Some((cb, acc)) = &cur {
        flush(*cb, acc, &mut out);
    }
    out
}

/// `bot preview`: a week of believable readings ending now. Days busier than nights, a quiet
/// night, evenings with a stream, and one spike two days back (an image build after a deploy).
pub fn seed(ctx: &Ctx, now: i64) -> Result<()> {
    let minute = 60_000;
    let end = now - now % minute;
    let start = end - WEEK_MS + minute;
    let mut conn = ctx.db.lock();
    let tx = conn.transaction()?;
    let total_mem = 15_820_000i64; // KiB, a 16 GB machine
    let disk_total = 238_000i64; // MiB, a 256 GB drive
    let boot = end - 12 * 86_400_000 - 5 * 3_600_000;
    let spike = (end - 2 * 86_400_000 - 3 * 3_600_000)..(end - 2 * 86_400_000 - 3 * 3_600_000 + 26 * minute);
    let mut at = start;
    let mut i: i64 = 0;
    while at <= end {
        // Hours in the owner's evening: the preview pretends to be in America/Toronto (UTC-5).
        let hour = ((at / 3_600_000 - 5).rem_euclid(24)) as f64 + (at % 3_600_000) as f64 / 3_600_000.0;
        let noise = |k: i64| -> f64 {
            let x = (i.wrapping_mul(2_654_435_761).wrapping_add(k * 40_503)) as u64;
            ((x ^ (x >> 13)).wrapping_mul(0x5bd1_e995) >> 8 & 0xffff) as f64 / 65_535.0
        };
        // 0 at 4 am, 1 at 4 pm.
        let day = 0.5 - 0.5 * ((hour - 4.0) / 24.0 * std::f64::consts::TAU).cos();
        // One evening and night off, three days back: no stream, nearly idle.
        let local_day = (at - 5 * 3_600_000).div_euclid(86_400_000);
        let off = (end - 5 * 3_600_000 - 3 * 86_400_000).div_euclid(86_400_000);
        let quiet = (local_day == off && hour >= 18.0) || (local_day == off + 1 && hour < 9.0);
        let calm = if quiet { 0.45 } else { 1.0 };
        // Evenings with a stream, ramping up and down over half an hour.
        let stream = if quiet { 0.0 } else { ((hour - 19.5) * 2.0).clamp(0.0, 1.0) * ((23.0 - hour) * 2.0).clamp(0.0, 1.0) };
        // In percent here, stored in hundredths.
        let mut cpu = (1.0 + 4.5 * day + noise(1) * 1.8) * calm + stream * (3.5 + noise(3) * 2.0);
        let mut bot_cpu = (0.06 + 0.22 * day + noise(2) * 0.08) * calm + stream * (0.45 + noise(4) * 0.25);
        let mut mem = 3_150_000.0 + day * 650_000.0 + noise(5) * 120_000.0 + stream * 250_000.0;
        let mut bot_mem = 118_000.0 + day * 16_000.0 + noise(6) * 4_000.0 + stream * 9_000.0;
        let mut temp = 340.0 + cpu * 9.0 + noise(7) * 12.0;
        if spike.contains(&at) {
            cpu = 70.0 + noise(8) * 18.0;
            mem += 3_400_000.0;
            temp = 740.0 + noise(9) * 30.0;
            bot_cpu += 0.3;
        }
        // The bot restarted with the deploy: its memory starts low and climbs back.
        if at >= spike.end {
            let since = ((at - spike.end) / minute) as f64;
            bot_mem -= 30_000.0 * (-since / 90.0).exp();
        }
        let s = Sample {
            at,
            cpu: Some((cpu * 100.0).round() as i64),
            mem_used: Some(mem.round() as i64),
            mem_total: Some(total_mem),
            disk_used: Some(61_400 + (at - start) / (WEEK_MS / 900)),
            disk_total: Some(disk_total),
            // Six cores: a load of 6 is every core busy.
            load1: Some((cpu * 6.0 + noise(10) * 20.0).round() as i64),
            uptime: Some((at - boot) / 1000),
            temp: Some(temp.round() as i64),
            bot_cpu: Some((bot_cpu * 100.0).round() as i64),
            bot_mem: Some(bot_mem.round() as i64),
        };
        insert(&tx, &s)?;
        at += minute;
        i += 1;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_average_what_was_read() {
        let m = 60_000;
        let rows = [
            (0, [Some(10), None]),
            (m, [Some(20), Some(5)]),
            (2 * m, [Some(31), None]),
            // 3 to 5 min: nothing (the bot was down).
            (6 * m, [Some(7), Some(8)]),
            (7 * m + 59_999, [Some(8), Some(9)]),
        ];
        let out = downsample(&rows, 3 * m);
        assert_eq!(out, vec![(0, [Some(20), Some(5)]), (6 * m, [Some(8), Some(9)])]);
        // 7.5 rounds up, so a bucket of 7 and 8 reads 8 (halves away from zero).
        let out = downsample(&rows[3..], 5 * m);
        assert_eq!(out, vec![(5 * m, [Some(8), Some(9)])]);
    }

    #[test]
    fn buckets_line_up_on_the_clock() {
        let m = 60_000;
        let rows: Vec<(i64, [Option<i64>; 1])> = (0..60).map(|i| (999_999_900_000 + i * m, [Some(i)])).collect();
        let out = downsample(&rows, 15 * m);
        assert!(out.iter().all(|(b, _)| b % (15 * m) == 0));
        // An hour from a whole hour is four quarters: 0 to 14 averages 7.
        assert_eq!(out.len(), 4);
        assert_eq!(out[0].1, [Some(7)]);
        assert!(downsample::<1>(&[], 15 * m).is_empty());
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn a_week_is_kept_and_no_more() {
        let ctx = bot_core::Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.db.migrate("server", MIGRATIONS).unwrap();
        let now = 2_000_000_000_000;
        seed(&ctx, now).unwrap();
        let all = since(&ctx, 0).unwrap();
        assert_eq!(all.len(), 7 * 1440);
        let newest = latest(&ctx).unwrap().unwrap();
        assert_eq!(newest.at, all.last().unwrap().at);
        record(&ctx, &Sample { at: newest.at + 60_000, cpu: Some(5), ..Sample::default() }).unwrap();
        // The oldest minute fell out of the week.
        assert_eq!(since(&ctx, 0).unwrap().len(), 7 * 1440);
        assert_eq!(latest(&ctx).unwrap().unwrap().cpu, Some(5));
        assert_eq!(latest(&ctx).unwrap().unwrap().mem_used, None);
    }
}
