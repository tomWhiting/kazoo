load("@build//buck:checks.bzl", "file_check")

load("@build//buck:drift.bzl", "generated_check")

generated_check(
    name = "check-generated",
)
file_check(
    kind = "length",
    logical = ".editorconfig",
    name = "check-length-.editorconfig",
    port_config = "",
    scanner = "toolchains//:tokei",
    src = ".editorconfig",
)
file_check(
    kind = "length",
    logical = "CONVENTIONS.toml",
    name = "check-length-CONVENTIONS.toml",
    port_config = "",
    scanner = "toolchains//:tokei",
    src = "CONVENTIONS.toml",
)

file_check(
    kind = "portable",
    logical = "CONVENTIONS.toml",
    name = "check-portable-CONVENTIONS.toml",
    port_config = "",
    scanner = "toolchains//:ast-grep",
    src = "CONVENTIONS.toml",
)
file_check(
    kind = "length",
    logical = "Cargo.toml",
    name = "check-length-Cargo.toml",
    port_config = "",
    scanner = "toolchains//:tokei",
    src = "Cargo.toml",
)

file_check(
    kind = "portable",
    logical = "Cargo.toml",
    name = "check-portable-Cargo.toml",
    port_config = "",
    scanner = "toolchains//:ast-grep",
    src = "Cargo.toml",
)

file_check(
    kind = "registry",
    logical = "Cargo.toml",
    name = "check-registry-Cargo.toml",
    owned_crates = [
        "kazoo-303",
        "kazoo-808",
        "kazoo-arp",
        "kazoo-core",
        "kazoo-cs80",
        "kazoo-dx",
        "kazoo-fx",
        "kazoo-glass",
        "kazoo-host",
        "kazoo-juno",
        "kazoo-mcp",
        "kazoo-mini",
        "kazoo-mix",
        "kazoo-mouth",
        "kazoo-perc",
        "kazoo-play",
        "kazoo-prophet",
        "kazoo-sampler",
        "kazoo-speech",
        "kazoo-string",
        "kazoo-tree",
        "kazoo-tui",
        "kazoo-wall",
        "kzhz",
        "kzoo"
    ],
    port_config = "",
    src = "Cargo.toml",
)
file_check(
    kind = "length",
    logical = "LICENSE",
    name = "check-length-LICENSE",
    port_config = "",
    scanner = "toolchains//:tokei",
    src = "LICENSE",
)
file_check(
    kind = "portable",
    logical = "ast-grep/rules/no-discarded-result.yml",
    name = "check-portable-ast-grep__rules__no-discarded-result.yml",
    port_config = "",
    scanner = "toolchains//:ast-grep",
    src = "ast-grep/rules/no-discarded-result.yml",
)
file_check(
    kind = "portable",
    logical = "ast-grep/rules/no-empty-error-arm.yml",
    name = "check-portable-ast-grep__rules__no-empty-error-arm.yml",
    port_config = "",
    scanner = "toolchains//:ast-grep",
    src = "ast-grep/rules/no-empty-error-arm.yml",
)
file_check(
    kind = "portable",
    logical = "ast-grep/rules/no-ignored-error-if-let.yml",
    name = "check-portable-ast-grep__rules__no-ignored-error-if-let.yml",
    port_config = "",
    scanner = "toolchains//:ast-grep",
    src = "ast-grep/rules/no-ignored-error-if-let.yml",
)
file_check(
    kind = "portable",
    logical = "ast-grep/rules/no-lint-bypass.yml",
    name = "check-portable-ast-grep__rules__no-lint-bypass.yml",
    port_config = "",
    scanner = "toolchains//:ast-grep",
    src = "ast-grep/rules/no-lint-bypass.yml",
)
file_check(
    kind = "portable",
    logical = "ast-grep/rules/no-result-ok.yml",
    name = "check-portable-ast-grep__rules__no-result-ok.yml",
    port_config = "",
    scanner = "toolchains//:ast-grep",
    src = "ast-grep/rules/no-result-ok.yml",
)
file_check(
    kind = "length",
    logical = "clippy.toml",
    name = "check-length-clippy.toml",
    port_config = "",
    scanner = "toolchains//:tokei",
    src = "clippy.toml",
)

file_check(
    kind = "portable",
    logical = "clippy.toml",
    name = "check-portable-clippy.toml",
    port_config = "",
    scanner = "toolchains//:ast-grep",
    src = "clippy.toml",
)
file_check(
    kind = "length",
    logical = "design/wall/handoff/wall.py",
    name = "check-length-design__wall__handoff__wall.py",
    port_config = "",
    scanner = "toolchains//:tokei",
    src = "design/wall/handoff/wall.py",
)

file_check(
    kind = "portable",
    logical = "design/wall/handoff/wall.py",
    name = "check-portable-design__wall__handoff__wall.py",
    port_config = "",
    scanner = "toolchains//:ast-grep",
    src = "design/wall/handoff/wall.py",
)
file_check(
    kind = "length",
    logical = "npm/.gitignore",
    name = "check-length-npm__.gitignore",
    port_config = "",
    scanner = "toolchains//:tokei",
    src = "npm/.gitignore",
)
file_check(
    kind = "length",
    logical = "npm/kzoo-darwin-arm64/.gitignore",
    name = "check-length-npm__kzoo-darwin-arm64__.gitignore",
    port_config = "",
    scanner = "toolchains//:tokei",
    src = "npm/kzoo-darwin-arm64/.gitignore",
)
file_check(
    kind = "length",
    logical = "npm/kzoo-darwin-x64/.gitignore",
    name = "check-length-npm__kzoo-darwin-x64__.gitignore",
    port_config = "",
    scanner = "toolchains//:tokei",
    src = "npm/kzoo-darwin-x64/.gitignore",
)
file_check(
    kind = "length",
    logical = "npm/kzoo/.gitignore",
    name = "check-length-npm__kzoo__.gitignore",
    port_config = "",
    scanner = "toolchains//:tokei",
    src = "npm/kzoo/.gitignore",
)
file_check(
    kind = "length",
    logical = "npm/kzoo/bin/kazoo.js",
    name = "check-length-npm__kzoo__bin__kazoo.js",
    port_config = "",
    scanner = "toolchains//:tokei",
    src = "npm/kzoo/bin/kazoo.js",
)

file_check(
    kind = "portable",
    logical = "npm/kzoo/bin/kazoo.js",
    name = "check-portable-npm__kzoo__bin__kazoo.js",
    port_config = "",
    scanner = "toolchains//:ast-grep",
    src = "npm/kzoo/bin/kazoo.js",
)
file_check(
    kind = "length",
    logical = "npm/kzoo/bin/kzhz.js",
    name = "check-length-npm__kzoo__bin__kzhz.js",
    port_config = "",
    scanner = "toolchains//:tokei",
    src = "npm/kzoo/bin/kzhz.js",
)

file_check(
    kind = "portable",
    logical = "npm/kzoo/bin/kzhz.js",
    name = "check-portable-npm__kzoo__bin__kzhz.js",
    port_config = "",
    scanner = "toolchains//:ast-grep",
    src = "npm/kzoo/bin/kzhz.js",
)
file_check(
    kind = "length",
    logical = "npm/kzoo/bin/kzoo.js",
    name = "check-length-npm__kzoo__bin__kzoo.js",
    port_config = "",
    scanner = "toolchains//:tokei",
    src = "npm/kzoo/bin/kzoo.js",
)

file_check(
    kind = "portable",
    logical = "npm/kzoo/bin/kzoo.js",
    name = "check-portable-npm__kzoo__bin__kzoo.js",
    port_config = "",
    scanner = "toolchains//:ast-grep",
    src = "npm/kzoo/bin/kzoo.js",
)
file_check(
    kind = "length",
    logical = "npm/kzoo/bin/launch.js",
    name = "check-length-npm__kzoo__bin__launch.js",
    port_config = "",
    scanner = "toolchains//:tokei",
    src = "npm/kzoo/bin/launch.js",
)

file_check(
    kind = "portable",
    logical = "npm/kzoo/bin/launch.js",
    name = "check-portable-npm__kzoo__bin__launch.js",
    port_config = "",
    scanner = "toolchains//:ast-grep",
    src = "npm/kzoo/bin/launch.js",
)
file_check(
    kind = "length",
    logical = "npm/notices.py",
    name = "check-length-npm__notices.py",
    port_config = "",
    scanner = "toolchains//:tokei",
    src = "npm/notices.py",
)

file_check(
    kind = "portable",
    logical = "npm/notices.py",
    name = "check-portable-npm__notices.py",
    port_config = "",
    scanner = "toolchains//:ast-grep",
    src = "npm/notices.py",
)
file_check(
    kind = "length",
    logical = "npm/publish.sh",
    name = "check-length-npm__publish.sh",
    port_config = "",
    scanner = "toolchains//:tokei",
    src = "npm/publish.sh",
)

file_check(
    kind = "portable",
    logical = "npm/publish.sh",
    name = "check-portable-npm__publish.sh",
    port_config = "",
    scanner = "toolchains//:ast-grep",
    src = "npm/publish.sh",
)
file_check(
    kind = "length",
    logical = "release/crates.sh",
    name = "check-length-release__crates.sh",
    port_config = "",
    scanner = "toolchains//:tokei",
    src = "release/crates.sh",
)

file_check(
    kind = "portable",
    logical = "release/crates.sh",
    name = "check-portable-release__crates.sh",
    port_config = "",
    scanner = "toolchains//:ast-grep",
    src = "release/crates.sh",
)
file_check(
    kind = "length",
    logical = "rustfmt.toml",
    name = "check-length-rustfmt.toml",
    port_config = "",
    scanner = "toolchains//:tokei",
    src = "rustfmt.toml",
)

file_check(
    kind = "portable",
    logical = "rustfmt.toml",
    name = "check-portable-rustfmt.toml",
    port_config = "",
    scanner = "toolchains//:ast-grep",
    src = "rustfmt.toml",
)
file_check(
    kind = "portable",
    logical = "sgconfig.yml",
    name = "check-portable-sgconfig.yml",
    port_config = "",
    scanner = "toolchains//:ast-grep",
    src = "sgconfig.yml",
)
