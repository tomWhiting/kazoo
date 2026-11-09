"""Host executables are discovered into ignored machine-local config."""
load("@prelude//rust:rust_toolchain.bzl", "PanicRuntime", "RustToolchainInfo")

def _impl(_ctx):
    return [
        DefaultInfo(),
        RustToolchainInfo(
            compiler = RunInfo(args = [read_root_config("host_tools", "rustc")]),
            clippy_driver = RunInfo(args = [read_root_config("host_tools", "clippy")]),
            rustdoc = RunInfo(args = [read_root_config("host_tools", "rustdoc")]),
            rustc_target_triple = read_root_config("host_tools", "rust_target"),
            default_edition = "2024",
            panic_runtime = PanicRuntime("unwind"),
            nightly_features = False,
        ),
    ]

host_rust_toolchain = rule(impl = _impl, attrs = {}, is_toolchain_rule = True)
