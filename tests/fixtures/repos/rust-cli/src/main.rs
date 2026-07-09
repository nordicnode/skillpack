use std::env;
fn main() {
 let a: Vec<String> = env::args().collect();
 if a.len() > 1 && a[1] == "--help" { println!("Usage: sample-rust [--new <entry>] [--verbose] [--version]"); return; }
 println!("sample-rust");
}
