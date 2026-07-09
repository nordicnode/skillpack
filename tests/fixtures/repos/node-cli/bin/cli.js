#!/usr/bin/env node
const a = process.argv.slice(2);
if (a.includes("--help")) { console.log("Usage: sample-node [--build] [--watch] [--port <n>]"); process.exit(0); }
console.log("sample-node");
