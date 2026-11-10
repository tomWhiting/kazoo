# Rust build graph

Cargo manifests and lockfiles remain authoritative. Cargo is also the editor
loop. Buck2 loads the shared build rules from the Git commit in `.buckconfig`;
no local shared-home path belongs in that file.

`buckgen.json` adds the independent `kazoo-kzhz` workspace to the main workspace.
The generated inventory is `buckgen/test-targets.txt`. Each Cargo integration
test file has its own Buck target. Length, portability and registry checks are
Buck tests with cached per-file reports. npm JavaScript packaging is outside
this conversion.

From the pinned shared build checkout, with `KAZOO_CHECKOUT` set by the operator:

```sh
go run ./buck/bucksetup --root "$KAZOO_CHECKOUT"
go run ./buck/buckgen --root "$KAZOO_CHECKOUT" --third-party
```

Bootstrap requires Rust, Go, Reindeer, tokei, ast-grep, a native C/C++ toolchain
and Python supported by the shared bootstrap. Host paths and helper binaries
are generated into ignored local files.

Then, from this checkout, using the provisioned Reindeer and Buck2:

```sh
reindeer --third-party-dir third-party vendor
reindeer --third-party-dir third-party buckify
toolchains/_local/buckgen
toolchains/_local/buckgen --check
buck2 build //...
buck2 test //...
```

Never edit generated BUCK files. Regenerate after Cargo changes and commit the
result. `//:check-generated` checks both the generator and Reindeer output for
drift without rewriting files.

NativeLink cache and worker-pool configuration comes from deployment discovery
and stays machine-local. Use the discovery launcher when selecting the cache
and pool. The shared scaffold selects local execution, which is also supported
on the build machine.
