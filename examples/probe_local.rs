//! Sample the local JVM the way the System tab does, and print what came back.
//! Run with: cargo run --example probe_local -- [pid]

use mctop::config::{JvmConfig, ProcessConfig, Tool};
use mctop::format;
use mctop::source::jvm::HeapWatcher;
use mctop::source::process::ProcessWatcher;

#[tokio::main]
async fn main() {
    let pid: Option<u32> = std::env::args().nth(1).and_then(|a| a.parse().ok());

    let mut process = ProcessWatcher::new(&ProcessConfig {
        pid,
        match_pattern: r"(?i)fakeserver|folia|paper|server\.jar".into(),
        enabled: true,
    })
    .unwrap();
    // A second argument is a wrapper to run the JDK tools through, e.g.
    //   cargo run --example probe_local -- 1234 "sudo -n -u minecraft"
    let wrapper: Vec<String> = std::env::args()
        .nth(2)
        .map(|w| w.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default();

    let tool = |name: &str| {
        let mut words = wrapper.clone();
        words.push(name.to_owned());
        Tool::from_words(words).unwrap()
    };

    let jvm = if wrapper.is_empty() {
        JvmConfig::default()
    } else {
        JvmConfig {
            jstat: tool("jstat"),
            jcmd: tool("jcmd"),
            ..JvmConfig::default()
        }
    };
    println!("jstat = {}", jvm.jstat.label());
    let mut heap = HeapWatcher::new(&jvm);

    // CPU is a delta between samples, so the first one is always zero.
    for round in 0..3 {
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

        let Some(stats) = process.sample() else {
            println!("round {round}: no matching process");
            continue;
        };

        println!(
            "round {round}: pid {} cpu {:.1}% ({} of machine) rss {} threads {:?} up {}",
            stats.pid,
            stats.cpu_percent,
            format::percent(stats.cpu_fraction()),
            format::bytes(stats.rss),
            stats.threads,
            format::duration(stats.uptime),
        );

        match heap.sample(stats.pid).await {
            Some(h) => println!(
                "          heap used {} / committed {} / max {}\n\
                 \x20         after GC {} ({}) · young {:?} full {:?} · gc time {:?} load {:?}\n\
                 \x20         non-heap {} · pressure {}",
                format::optional(h.used, format::bytes),
                format::optional(h.committed, format::bytes),
                format::optional(h.max, format::bytes),
                format::optional(h.after_gc, format::bytes),
                if h.after_gc_measured {
                    "measured"
                } else {
                    "estimated"
                },
                h.young_collections,
                h.full_collections,
                h.gc_seconds,
                h.gc_load,
                format::optional(h.non_heap, format::bytes),
                format::optional(h.pressure(), format::percent),
            ),
            None => println!("          heap unavailable: {:?}", heap.last_error),
        }
    }
}
