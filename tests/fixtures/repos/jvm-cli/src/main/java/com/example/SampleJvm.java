package com.example;

public class SampleJvm {
    public static void main(String[] args) {
        if (args.length > 0 && args[0].equals("--help")) {
            System.out.println("Usage: sample-jvm [--new <entry>] [--verbose]");
            System.exit(0);
        }
        System.out.println("sample-jvm");
    }
}
