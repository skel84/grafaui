//! Prints how well dashboards are supported: `cargo run -p grafaui-model
//! --example support -- a.json b.json …`.
//!
//! One line per dashboard (full / partial / placeholder panel counts), then
//! every ignored feature and placeholder type across all of them, by how
//! many panels it affects.

use std::collections::BTreeMap;

use grafaui_model::{Dashboard, Support, Viz};

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    assert!(!paths.is_empty(), "usage: support <dashboard.json>…");
    let mut reasons: BTreeMap<String, (usize, Vec<String>)> = BTreeMap::new();
    let mut total = (0, 0, 0);
    for path in &paths {
        let name = path.rsplit('/').next().unwrap_or(path);
        let json = std::fs::read_to_string(path).expect("readable file");
        let dashboard = match Dashboard::parse(&json) {
            Ok(dashboard) => dashboard,
            Err(error) => {
                println!("{name:<16} FAILED TO PARSE: {error}");
                continue;
            }
        };
        let (full, partial, placeholder) = dashboard.support_counts();
        total = (total.0 + full, total.1 + partial, total.2 + placeholder);
        println!(
            "{name:<16} {full:>3} full {partial:>3} partial {placeholder:>3} placeholder  {}",
            dashboard.title
        );
        let mut seen = |reason: String| {
            let entry = reasons.entry(reason).or_default();
            entry.0 += 1;
            if !entry.1.iter().any(|n| n == name) {
                entry.1.push(name.to_owned());
            }
        };
        for panel in &dashboard.panels {
            if panel.support() == Support::Placeholder || matches!(panel.viz, Viz::Unsupported) {
                seen(format!("placeholder: {}", panel.kind));
            }
            for note in &panel.ignored {
                seen(note.clone());
            }
        }
    }
    println!(
        "\nall: {} full, {} partial, {} placeholder\n",
        total.0, total.1, total.2
    );
    let mut reasons: Vec<_> = reasons.into_iter().collect();
    reasons.sort_by_key(|(_, (count, _))| std::cmp::Reverse(*count));
    for (reason, (count, files)) in reasons {
        println!("{count:4}  {reason}  [{}]", files.join(" "));
    }
}
