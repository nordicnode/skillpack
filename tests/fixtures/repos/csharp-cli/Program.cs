// Sample C# CLI for skillpack integration testing.
// Prints a --help message so the verify CLI-invocation check passes.
using System;
using System.Linq;

if (args.Contains("--help"))
{
    Console.WriteLine("Usage: sample-csharp [--new <entry>] [--verbose]");
    return 0;
}
Console.WriteLine("sample-csharp");
return 0;
