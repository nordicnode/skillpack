if (Deno.args[0] === "--help") {
  console.log("Usage: sample-deno [--new <entry>] [--verbose]");
  Deno.exit(0);
}
if (Deno.args[0] === "--version") {
  console.log("sample-deno 0.1.0");
  Deno.exit(0);
}
console.log("sample-deno");
