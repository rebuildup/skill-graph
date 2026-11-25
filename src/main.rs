fn main() {
    std::process::exit(match skill_graph::cli::run() {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("{err}");
            2
        }
    });
}
