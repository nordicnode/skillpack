const std = @import("std");

pub fn build(b: *std.Build) void {
    _ = b.addExecutable(.{
        .name = "sample-zig",
        .root_source_file = b.path("src/main.zig"),
        .target = b.standardTargetOptions(.{}),
        .optimize = .Debug,
    });
}
