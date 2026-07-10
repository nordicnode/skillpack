/// Hand-rolled clap-shaped CLI for the subcommand-drift e2e fixture.
/// Zero deps: prints a `Commands:` section that `extract_subcommands` parses,
/// and per-subcommand `--help` that `spawn_capture` + `check_subcommand_drift`
/// exercise end-to-end. Mirrors the `rust-cli`/`bad-help` fixture style.
use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();
    // `sample-sub --help`
    if args.len() == 2 && args[1] == "--help" {
        println!(
            "Usage: sample-sub [OPTIONS] <COMMAND>\n\
             \n\
             Commands:\n\
             \x20 init    Scaffold the distribution layer\n\
             \x20 verify  Check the distribution files\n\
             \x20 help    Print this message or the help of the given subcommand(s)\n\
             \n\
             Options:\n\
             \x20     --root <DIR>  Project root to operate on\n\
             \x20 -h, --help        Print help\n\
             \x20 -V, --version     Print version"
        );
        return;
    }
    // `sample-sub <sub> --help`
    if args.len() == 3 && args[2] == "--help" {
        match args[1].as_str() {
            "init" => {
                println!(
                    "Usage: sample-sub init [OPTIONS]\n\
                     \n\
                     Options:\n\
                     \x20     --root <DIR>           Project root to operate on\n\
                     \x20     --non-interactive      Skip interactive prompts\n\
                     \x20     --accept-warnings      Accept verification warnings\n\
                     \x20 -h, --help                 Print help"
                );
                return;
            }
            "verify" => {
                println!(
                    "Usage: sample-sub verify [OPTIONS]\n\
                     \n\
                     Options:\n\
                     \x20     --root <DIR>   Project root to operate on\n\
                     \x20     --format <FMT> Output format (human or json)\n\
                     \x20 -h, --help        Print help"
                );
                return;
            }
            _ => {}
        }
    }
    println!("sample-sub");
}
