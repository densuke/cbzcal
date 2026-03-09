use clap::Parser;

use cbzcal::{cli::Cli, execute};

fn main() {
    let cli = Cli::parse();

    match execute(cli) {
        Ok(output) => println!("{output}"),
        Err(error) => {
            eprintln!("{error:#}");
            std::process::exit(1);
        }
    }
}
