//! Prints a dashboard's sections and panel positions: `cargo run -p
//! grafaui-model --example layout -- dashboard.json`.

use grafaui_model::Dashboard;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: layout <dashboard.json>");
    let dashboard =
        Dashboard::parse(&std::fs::read_to_string(path).expect("readable")).expect("a dashboard");
    for section in &dashboard.sections {
        let title = section.row.as_ref().map_or("(top)", |r| r.title.as_str());
        println!(
            "{title}: height {} rows, {} panels",
            section.height,
            section.panels.len()
        );
        for placed in section.panels.iter().take(3) {
            println!("    {:>3} {:?}", placed.key, placed.pos);
        }
    }
}
