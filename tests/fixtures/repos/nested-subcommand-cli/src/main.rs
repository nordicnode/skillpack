/// Hand-rolled clap-shaped CLI for the nested-subcommand-drift e2e fixture.
/// Zero deps: the top-level `--help` advertises `remote`, whose own `--help`
/// advertises `add`/`remove`, whose own `--help` exposes distinct flags. This
/// exercises `capture_subcommand_tree` (recursive introspect) and
/// `check_subcommand_drift` (verify recurses `<base> <path...> --help`).
use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();

    // `sample-nested --help`
    if args.len() == 2 && args[1] == "--help" {
        println!(
            "Usage: sample-nested [OPTIONS] <COMMAND>\n\
             \n\
             Commands:\n\
             \x20 remote  Manage remotes\n\
             \x20 help    Print this message or the help of the given subcommand(s)\n\
             \n\
             Options:\n\
             \x20 -h, --help     Print help\n\
             \x20 -V, --version  Print version"
        );
        return;
    }

    // `sample-nested remote --help`
    if args.len() == 3 && args[1] == "remote" && args[2] == "--help" {
        println!(
            "Usage: sample-nested remote <COMMAND>\n\
             \n\
             Commands:\n\
             \x20 add     Add a remote\n\
             \x20 remove  Remove a remote\n\
             \x20 help    Print this message or the help of the given subcommand(s)"
        );
        return;
    }

    // `sample-nested remote <sub> --help`
    if args.len() == 4 && args[1] == "remote" && args[3] == "--help" {
        match args[2].as_str() {
            "add" => {
                println!(
                    "Usage: sample-nested remote add [OPTIONS]\n\
                     \n\
                     Options:\n\
                     \x20     --name <NAME>  Name of the remote\n\
                     \x20     --url <URL>    URL of the remote\n\
                     \x20 -h, --help         Print help"
                );
                return;
            }
            "remove" => {
                println!(
                    "Usage: sample-nested remote remove [OPTIONS]\n\
                     \n\
                     Options:\n\
                     \x20     --name <NAME>  Name of the remote to remove\n\
                     \x20 -h, --help         Print help"
                );
                return;
            }
            _ => {}
        }
    }

    println!("sample-nested");
}
