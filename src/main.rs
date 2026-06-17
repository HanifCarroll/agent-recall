fn main() {
    if let Err(error) = agent_recall::run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
