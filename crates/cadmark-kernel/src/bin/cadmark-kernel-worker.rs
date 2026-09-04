// The confined process that runs build123d scripts. Started by the
// application's `KernelWorker`; never run by hand.

fn main() {
    env_logger::init();
    let args = match cadmark_kernel::worker::WorkerArgs::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(error) => {
            println!("refused: {error}");
            std::process::exit(2);
        }
    };
    std::process::exit(cadmark_kernel::worker::run_worker(args));
}
