// The confined process that runs build123d scripts. Started by the
// application's `KernelWorker`; never run by hand.

fn main() {
    let args = match cadmark_kernel::worker::WorkerArgs::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(error) => {
            println!("refused: {error}");
            std::process::exit(2);
        }
    };
    // The environment is empty by design, so the log filter arrives as an
    // argument rather than through RUST_LOG.
    let mut logger = env_logger::Builder::new();
    if let Some(filter) = &args.log_filter {
        logger.parse_filters(filter);
    }
    logger.init();
    std::process::exit(cadmark_kernel::worker::run_worker(args));
}
