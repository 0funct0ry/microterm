#![allow(dead_code)]
use std::collections::HashMap;

use chrono::{DateTime, FixedOffset, TimeZone};
use mt_snippets::{expand, Context, Expanded, Fact, Mode, RuntimeError, Session, Template, Val};
use rand::{RngCore, SeedableRng};

pub struct Mock {
    pub rng: rand::rngs::StdRng,
    pub facts: HashMap<String, String>,
    pub env: HashMap<String, String>,
    pub counters: HashMap<String, i64>,
}

impl Mock {
    pub fn new() -> Self {
        Self {
            rng: rand::rngs::StdRng::seed_from_u64(42),
            facts: HashMap::new(),
            env: HashMap::new(),
            counters: HashMap::new(),
        }
    }
    pub fn fact(mut self, f: Fact, v: &str) -> Self {
        self.facts.insert(format!("{f:?}"), v.into());
        self
    }
    pub fn env(mut self, k: &str, v: &str) -> Self {
        self.env.insert(k.into(), v.into());
        self
    }
}

pub fn fixed_now() -> DateTime<FixedOffset> {
    FixedOffset::east_opt(2 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 3, 4, 5, 6, 7)
        .unwrap()
}

impl Context for Mock {
    fn now(&self) -> DateTime<FixedOffset> {
        fixed_now()
    }
    fn fill_random(&mut self, buf: &mut [u8]) {
        self.rng.fill_bytes(buf);
    }
    fn env(&self, name: &str) -> Option<String> {
        self.env.get(name).cloned()
    }
    fn fact(&mut self, f: &Fact) -> Result<String, String> {
        self.facts
            .get(&format!("{f:?}"))
            .cloned()
            .ok_or_else(|| format!("{f:?} is unavailable"))
    }
    fn counter_get(&self, name: &str) -> Option<i64> {
        self.counters.get(name).copied()
    }
    fn counter_commit(&mut self, name: &str, next: i64) {
        self.counters.insert(name.into(), next);
    }
}

pub fn run(src: &str) -> Result<String, RuntimeError> {
    run_with(src, &mut Mock::new())
}

pub fn run_with(src: &str, ctx: &mut Mock) -> Result<String, RuntimeError> {
    let t = Template::compile(src);
    let mut s = Session::new(ctx);
    expand(&t, ctx, &mut s, Mode::Insert).map(|e| e.text)
}

pub fn full(
    src: &str,
    ctx: &mut Mock,
    vals: &[(&str, Val)],
    mode: Mode,
) -> Result<Expanded, RuntimeError> {
    let t = Template::compile(src);
    let mut s = Session::new(ctx);
    for (k, v) in vals {
        s.set_prompt(k, v.clone());
    }
    expand(&t, ctx, &mut s, mode)
}
