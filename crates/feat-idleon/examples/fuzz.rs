//! Feed every check broken saves and look for panics (the release build aborts on one):
//! `cargo run --release -p feat-idleon --example fuzz -- save.json`. Each top-level key in turn
//! is replaced with junk of every shape, and every check runs on the result.

use std::panic;

use bot_core::serde_json::{self, Value, json};
use feat_idleon::review::Review;
use feat_idleon::save::Save;

fn main() {
    let junk = [
        json!(null), json!(-1), json!(1e308), json!(-1e308), json!("x"), json!("[1,2"), json!([]), json!({}),
        json!([[-5, "a", null], {"0": -1}]), Value::Array(vec![json!(1e300); 3]), json!({"length": -3, "0": "nan"}), json!(true),
        json!([[[[[[]]]]]]), Value::Array(vec![json!(-1); 700]), json!({"1000000": 5}),
        // A numbered key far past any real list: must not become a list billions long.
        json!({"4000000000": 5}), json!([{"4000000000": 5}, {"18446744073709551615": 1}]),
    ];
    panic::set_hook(Box::new(|_| {}));
    let mut failures = 0;
    for path in std::env::args().skip(1) {
        let root: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let data = root.get("data").cloned().unwrap_or(root.clone());
        let keys: Vec<String> = data.as_object().unwrap().keys().cloned().collect();
        let mut runs = 0;
        for k in &keys {
            for j in &junk {
                let mut d = data.clone();
                d[k] = j.clone();
                // Also as JSON written as text, the way the game stores most keys.
                for v in [j.clone(), Value::String(j.to_string())] {
                    d[k] = v;
                    let text = json!({"data": d, "charNames": ["a", "b"]}).to_string();
                    runs += 1;
                    let ok = panic::catch_unwind(|| {
                        if let Ok(s) = Save::parse(&text) {
                            let r = Review::new(&s, feat_idleon::run_checks(&s), feat_idleon::world_reached(&s));
                            std::hint::black_box(r.share());
                        }
                    });
                    if ok.is_err() {
                        failures += 1;
                        if failures < 30 {
                            eprintln!("PANIC with {k} = {}", d[k].to_string().chars().take(60).collect::<String>());
                        }
                    }
                }
            }
        }
        // Then each number inside a key (OptLacc[319], Spelunk[5][3]...), one at a time, at the
        // extremes: loops and casts that trust a count from the save show up here.
        let deep = std::env::var("FUZZ_DEEP").is_ok();
        if deep {
            for k in &keys {
                let mut decoded = data[k].clone();
                if let Value::String(t) = &decoded
                    && let Ok(v) = serde_json::from_str::<Value>(t)
                {
                    decoded = v;
                }
                let mut paths: Vec<Vec<usize>> = Vec::new();
                if let Value::Array(a) = &decoded {
                    for (i, x) in a.iter().enumerate().take(1000) {
                        paths.push(vec![i]);
                        if let Value::Array(b) = x {
                            paths.extend((0..b.len().min(120)).map(|j| vec![i, j]));
                        }
                    }
                }
                for p in &paths {
                    for big in [json!(1e18), json!(-1e18), json!(4294967296.5)] {
                        let mut v = decoded.clone();
                        let slot = p.iter().try_fold(&mut v, |acc, &i| acc.get_mut(i));
                        let Some(slot) = slot else { continue };
                        *slot = big;
                        let mut d = data.clone();
                        d[k] = v;
                        let text = json!({"data": d, "charNames": ["a", "b"]}).to_string();
                        runs += 1;
                        let t = std::time::Instant::now();
                        let ok = panic::catch_unwind(|| {
                            if let Ok(s) = Save::parse(&text) {
                                let r = Review::new(&s, feat_idleon::run_checks(&s), feat_idleon::world_reached(&s));
                                std::hint::black_box(r.share());
                            }
                        });
                        let slow = t.elapsed() > std::time::Duration::from_millis(500);
                        if ok.is_err() || slow {
                            failures += 1;
                            if failures < 30 {
                                eprintln!("{} with {k}{p:?} = {}", if slow { "SLOW" } else { "PANIC" }, d[k].to_string().chars().take(40).collect::<String>());
                            }
                        }
                    }
                }
            }
        }
        println!("{path}: {runs} broken saves, {failures} panics so far");
    }
    std::process::exit(i32::from(failures > 0));
}
