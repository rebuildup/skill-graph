use std::path::PathBuf;

fn main() {
    let dataset = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("evals/dataset/cases.jsonl"));
    match skill_graph::x02::run(&dataset) {
        Ok(value) => println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("serialize x02 report")
        ),
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    }
}
