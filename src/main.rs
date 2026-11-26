fn main() {
    std::process::exit(match skill_graph::cli::run() {
        Ok(()) => 0,
        Err(err) => {
            skill_graph::cli::write_stderr_line(&err.to_string());
            2
        }
    });
}
