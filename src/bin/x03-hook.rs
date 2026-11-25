use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
struct Args {
    query: String,
    #[arg(long)]
    root: PathBuf,
    #[arg(long, default_value_t = 3)]
    max: usize,
    #[arg(long)]
    context: Option<String>,
    #[arg(long)]
    text_only: bool,
}

fn main() {
    let args = Args::parse();
    match skill_graph::x03::recommend_names(
        &args.root,
        &args.query,
        args.context.as_deref().unwrap_or(""),
        args.max,
        args.text_only,
    ) {
        Ok(names) => println!(
            "{}",
            serde_json::to_string(&names).expect("serialize names")
        ),
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    }
}
